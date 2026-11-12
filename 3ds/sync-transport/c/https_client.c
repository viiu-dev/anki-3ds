// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

#include "https_client.h"
#include <3ds.h>
#include <curl/curl.h>
#include <malloc.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <strings.h>
#include <stdlib.h>
#include <sys/socket.h>
#include "dns.h"

#define CA_FILE "sdmc:/3ds/anki-3ds/cacert.pem"
#define MAX_RESPONSE_BYTES (16u * 1024u * 1024u)
#define IO_BUFFER_BYTES (64u * 1024u)
#define DNS_CACHE_MS (5u * 60u * 1000u)
#define HTTPS_SLOT_COUNT 5

struct ResponseBuffer {
    uint8_t *data;
    size_t size;
    size_t capacity;
    uint64_t deadline;
    uint32_t io_timeout_ms;
    bool timed_out;
    curl_off_t last_download;
    curl_off_t last_upload;
    uint32_t sampled_heap_peak;
    FILE *file;
    const Anki3dsRequestOptions *options;
    Anki3dsHttpResponse *resp;
    curl_off_t content_length;
};

static CURL *g_easy[HTTPS_SLOT_COUNT];
static CURLM *g_multi[HTTPS_SLOT_COUNT];

typedef struct {
    char host[128];
    char ip[16];
    uint64_t expires;
} DnsCacheEntry;

static DnsCacheEntry g_dns_cache[HTTPS_SLOT_COUNT][2];
static int g_dns_cache_next[HTTPS_SLOT_COUNT];

static size_t header_cb(char *data, size_t size, size_t count, void *context) {
    if (size && count > SIZE_MAX / size) return 0;
    size_t bytes = size * count;
    struct ResponseBuffer *buf = context;
    Anki3dsHttpResponse *response = buf->resp;
    if (bytes >= 5 && !strncmp(data, "HTTP/", 5)) {
        response->server_time = 0;
        buf->content_length = -1;
        if (buf->size > 0) {
            buf->size = 0;
            if (buf->options && buf->options->write_body) {
                buf->options->write_body(buf->options->context, NULL, 0, 0);
            }
        }
    }
    if (bytes > 5 && bytes < 128 && !strncasecmp(data, "Date:", 5)) {
        char value[128];
        memcpy(value, data + 5, bytes - 5);
        value[bytes - 5] = 0;
        time_t date = curl_getdate(value, NULL);
        if (date > 0) response->server_time = (int64_t)date;
    }
    if (bytes > 15 && !strncasecmp(data, "Content-Length:", 15)) {
        size_t i = 15;
        while (i < bytes && (data[i] == ' ' || data[i] == '\t')) i++;
        uint64_t length = 0;
        while (i < bytes && data[i] >= '0' && data[i] <= '9') {
            length = length * 10 + (uint64_t)(data[i] - '0');
            if (length > (uint64_t)SIZE_MAX) {
                length = (uint64_t)SIZE_MAX;
                break;
            }
            i++;
        }
        buf->content_length = (curl_off_t)(length > (uint64_t)INT64_MAX
            ? INT64_MAX : length);
    }
    return bytes;
}

static size_t write_cb(char *ptr, size_t size, size_t nmemb, void *userdata) {
    if (size && nmemb > SIZE_MAX / size) return 0;
    size_t bytes = size * nmemb;
    struct ResponseBuffer *buf = (struct ResponseBuffer *)userdata;

    if (buf->file) {
        if (buf->size > buf->options->max_file_bytes
            || bytes > buf->options->max_file_bytes - buf->size) return 0;
        size_t written = fwrite(ptr, 1, bytes, buf->file);
        buf->size += written;
        return written;
    }
    if (bytes == 0) return 0;
    if (buf->options && buf->options->write_body) {
        size_t hint = (buf->content_length > 0 && (uint64_t)buf->content_length <= MAX_RESPONSE_BYTES)
            ? (size_t)buf->content_length
            : 0;
        if (buf->options->write_body(buf->options->context, (const uint8_t *)ptr, bytes, hint) != 0)
            return 0;
        buf->size += bytes;
        return bytes;
    }
    if (bytes > MAX_RESPONSE_BYTES - buf->size) return 0;
    if (buf->size + bytes > buf->capacity) {
        size_t need = buf->size + bytes;            /* safe: bytes <= MAX - size */
        size_t new_cap = buf->capacity ? buf->capacity : IO_BUFFER_BYTES;
        if (buf->capacity == 0 && buf->content_length > 0
            && (uint64_t)buf->content_length <= MAX_RESPONSE_BYTES
            && (size_t)buf->content_length > new_cap) {
            new_cap = (size_t)buf->content_length;
        }
        while (new_cap < need) {
            if (new_cap > MAX_RESPONSE_BYTES / 2) { new_cap = MAX_RESPONSE_BYTES; break; }
            new_cap *= 2;
        }
        uint8_t *new_ptr = realloc(buf->data, new_cap);
        if (!new_ptr) return 0;
        buf->data = new_ptr;
        buf->capacity = new_cap;
    }

    memcpy(buf->data + buf->size, ptr, bytes);
    buf->size += bytes;
    return bytes;
}

static int xfer_cb(void *clientp, curl_off_t dltotal, curl_off_t dlnow, curl_off_t ultotal, curl_off_t ulnow) {
    (void)dltotal; (void)ultotal;
    struct ResponseBuffer *buf = (struct ResponseBuffer *)clientp;
    if (!buf || !buf->options) return 0;
    uint32_t heap = (uint32_t)mallinfo().uordblks;
    if (heap > buf->sampled_heap_peak) buf->sampled_heap_peak = heap;
    if (buf->options->cancelled && buf->options->cancelled(buf->options->context)) return 1;
    if (buf->options->progress && buf->options->progress(buf->options->context,
        (uint64_t)(dlnow + ulnow), (uint64_t)(dltotal + ultotal))) return 1;
    if (dlnow != buf->last_download || ulnow != buf->last_upload) {
        buf->last_download = dlnow;
        buf->last_upload = ulnow;
        buf->deadline = osGetTime() + buf->io_timeout_ms;
    }
    if (osGetTime() >= buf->deadline) {
        buf->timed_out = true;
        return 1;
    }
    return 0;
}

static bool dns_cancel(void *context) {
    const Anki3dsRequestOptions *options = context;
    return options->cancelled && options->cancelled(options->context);
}

static void dns_cache_clear(void) {
    memset(g_dns_cache, 0, sizeof(g_dns_cache));
    memset(g_dns_cache_next, 0, sizeof(g_dns_cache_next));
}

static void slot_teardown(int slot) {
    if (g_multi[slot]) { curl_multi_cleanup(g_multi[slot]); g_multi[slot] = NULL; }
    if (g_easy[slot])  { curl_easy_cleanup(g_easy[slot]);  g_easy[slot]  = NULL; }
}

static bool is_transient(int code) {
    return code == CURLE_COULDNT_CONNECT || code == CURLE_OPERATION_TIMEDOUT ||
           code == CURLE_SSL_CONNECT_ERROR || code == CURLE_GOT_NOTHING ||
           code == CURLE_SEND_ERROR || code == CURLE_RECV_ERROR;
}

static void dns_cache_forget(int slot, const char *host) {
    for (int i = 0; i < 2; i++) {
        if (g_dns_cache[slot][i].host[0] && strcmp(g_dns_cache[slot][i].host, host) == 0) {
            g_dns_cache[slot][i].host[0] = '\0';
            g_dns_cache[slot][i].ip[0] = '\0';
            g_dns_cache[slot][i].expires = 0;
        }
    }
}

static const char *dns_cache_lookup(int slot, const char *host) {
    uint64_t now = osGetTime();
    for (int i = 0; i < 2; i++) {
        if (g_dns_cache[slot][i].host[0] && g_dns_cache[slot][i].expires > now
            && strcmp(g_dns_cache[slot][i].host, host) == 0) {
            return g_dns_cache[slot][i].ip;
        }
    }
    return NULL;
}

static void dns_cache_store(int slot, const char *host, const char *ip) {
    DnsCacheEntry *entry = &g_dns_cache[slot][g_dns_cache_next[slot]];
    g_dns_cache_next[slot] = (g_dns_cache_next[slot] + 1) % 2;
    snprintf(entry->host, sizeof(entry->host), "%s", host);
    snprintf(entry->ip, sizeof(entry->ip), "%s", ip);
    entry->expires = osGetTime() + DNS_CACHE_MS;
}

int anki_3ds_https_global_init(void) {
    return (int)curl_global_init(CURL_GLOBAL_DEFAULT);
}

void anki_3ds_https_cleanup(void) {
    for (int slot = 0; slot < HTTPS_SLOT_COUNT; slot++) {
        slot_teardown(slot);
    }
    dns_cache_clear();
}

static int request_with_ip(
    int slot,
    const char *host,
    const char *ip,
    const char *path,
    const char *header_anki_sync,
    const uint8_t *body,
    size_t body_len,
    uint32_t timeout_ms,
    uint32_t connect_timeout_ms,
    const Anki3dsRequestOptions *options,
    Anki3dsHttpResponse *out_resp
) {
    if (!g_easy[slot]) {
        g_easy[slot] = curl_easy_init();
        if (!g_easy[slot]) {
            snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "curl_easy_init failed");
            return -3;
        }
    } else {
        curl_easy_reset(g_easy[slot]);
    }
    CURL *easy = g_easy[slot];

    char url[512];
    snprintf(url, sizeof(url), "https://%s:%u%s", host, (unsigned)options->port, path);

    char mapping[256];
    snprintf(mapping, sizeof(mapping), "%s:%u:%s", host, (unsigned)options->port, ip);

    struct curl_slist *resolve = curl_slist_append(NULL, mapping);
    struct curl_slist *headers = curl_slist_append(NULL, "Content-Type: application/octet-stream");
    if (header_anki_sync && header_anki_sync[0]) {
        char sync_hdr_line[512];
        snprintf(sync_hdr_line, sizeof(sync_hdr_line), "anki-sync: %s", header_anki_sync);
        headers = curl_slist_append(headers, sync_hdr_line);
    }

    struct ResponseBuffer rx_buf = {0};
    rx_buf.options = options;
    rx_buf.resp = out_resp;
    rx_buf.content_length = -1;
    rx_buf.io_timeout_ms = timeout_ms;
    rx_buf.deadline = osGetTime() + (uint64_t)timeout_ms;

    FILE *upload = NULL;
    if (options->upload_file) {
        upload = fopen(options->upload_file, "rb");
        if (!upload) {
            curl_slist_free_all(headers);
            curl_slist_free_all(resolve);
            return -4;
        }
        if (fseek(upload, 0, SEEK_END) || ftell(upload) < 0) {
            fclose(upload);
            curl_slist_free_all(headers);
            curl_slist_free_all(resolve);
            snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "cannot determine upload size");
            return -4;
        }
        body_len = (size_t)ftell(upload);
        if (body_len > options->max_file_bytes) {
            fclose(upload);
            curl_slist_free_all(headers);
            curl_slist_free_all(resolve);
            snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "compressed upload exceeds protocol limit");
            return -4;
        }
        rewind(upload);
    }
    if (options->download_file) {
        rx_buf.file = fopen(options->download_file, "wb");
        if (!rx_buf.file) {
            if (upload) fclose(upload);
            curl_slist_free_all(headers);
            curl_slist_free_all(resolve);
            return -4;
        }
    }

    curl_easy_setopt(easy, CURLOPT_URL, url);
    curl_easy_setopt(easy, CURLOPT_HEADERFUNCTION, header_cb);
    curl_easy_setopt(easy, CURLOPT_HEADERDATA, &rx_buf);
    curl_easy_setopt(easy, CURLOPT_RESOLVE, resolve);
    curl_easy_setopt(easy, CURLOPT_PROTOCOLS_STR, "https");
    curl_easy_setopt(easy, CURLOPT_REDIR_PROTOCOLS_STR, "https");
    curl_easy_setopt(easy, CURLOPT_FOLLOWLOCATION, 0L);
    curl_easy_setopt(easy, CURLOPT_HTTP_VERSION, (long)CURL_HTTP_VERSION_1_1);
    curl_easy_setopt(easy, CURLOPT_SSLVERSION, (long)CURL_SSLVERSION_TLSv1_2);
    curl_easy_setopt(easy, CURLOPT_SSL_VERIFYPEER, 1L);
    curl_easy_setopt(easy, CURLOPT_SSL_VERIFYHOST, 2L);
    curl_easy_setopt(easy, CURLOPT_SSL_SESSIONID_CACHE, 1L);
    curl_easy_setopt(easy, CURLOPT_CAINFO, options->ca_file);
    curl_easy_setopt(easy, CURLOPT_CONNECTTIMEOUT_MS, (long)connect_timeout_ms);
    curl_easy_setopt(easy, CURLOPT_TIMEOUT_MS, 0L);
    curl_easy_setopt(easy, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(easy, CURLOPT_BUFFERSIZE, (long)IO_BUFFER_BYTES);
    curl_easy_setopt(easy, CURLOPT_UPLOAD_BUFFERSIZE, (long)IO_BUFFER_BYTES);
    curl_easy_setopt(easy, CURLOPT_TCP_NODELAY, 1L);
    curl_easy_setopt(easy, CURLOPT_TCP_KEEPALIVE, 1L);
    curl_easy_setopt(easy, CURLOPT_TCP_KEEPIDLE, 30L);
    curl_easy_setopt(easy, CURLOPT_TCP_KEEPINTVL, 15L);
    curl_easy_setopt(easy, CURLOPT_FRESH_CONNECT, 0L);
    curl_easy_setopt(easy, CURLOPT_FORBID_REUSE, 0L);
    curl_easy_setopt(easy, CURLOPT_MAXCONNECTS, 1L);
    curl_easy_setopt(easy, CURLOPT_MAXAGE_CONN, 20L);
    curl_easy_setopt(easy, CURLOPT_WRITEFUNCTION, write_cb);
    curl_easy_setopt(easy, CURLOPT_WRITEDATA, &rx_buf);
    curl_easy_setopt(easy, CURLOPT_NOPROGRESS, 0L);
    curl_easy_setopt(easy, CURLOPT_XFERINFOFUNCTION, xfer_cb);
    curl_easy_setopt(easy, CURLOPT_XFERINFODATA, &rx_buf);
    curl_easy_setopt(easy, CURLOPT_HTTPHEADER, headers);
    curl_easy_setopt(easy, CURLOPT_USERAGENT, "Anki3DS/1.1");

    if (upload) {
        curl_easy_setopt(easy, CURLOPT_POST, 1L);
        curl_easy_setopt(easy, CURLOPT_READDATA, upload);
        curl_easy_setopt(easy, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)body_len);
    } else if (body && body_len > 0) {
        curl_easy_setopt(easy, CURLOPT_POST, 1L);
        curl_easy_setopt(easy, CURLOPT_POSTFIELDS, (const char *)body);
        curl_easy_setopt(easy, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)body_len);
    } else {
        curl_easy_setopt(easy, CURLOPT_POST, 1L);
        curl_easy_setopt(easy, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)0);
    }

    char errbuf[CURL_ERROR_SIZE] = {0};
    curl_easy_setopt(easy, CURLOPT_ERRORBUFFER, errbuf);

    CURLcode code = CURLE_OK;
    if (options->cancelled && options->cancelled(options->context)) {
        code = CURLE_ABORTED_BY_CALLBACK;
        snprintf(errbuf, sizeof(errbuf), "sync cancelled");
    } else {
        if (!g_multi[slot]) {
            g_multi[slot] = curl_multi_init();
        }
        CURLM *multi = g_multi[slot];
        if (!multi) {
            code = CURLE_FAILED_INIT;
            snprintf(errbuf, sizeof(errbuf), "curl_multi_init failed");
        } else if (curl_multi_add_handle(multi, easy) != CURLM_OK) {
            code = CURLE_FAILED_INIT;
            snprintf(errbuf, sizeof(errbuf), "curl_multi_add_handle failed");
        } else {
            int running = 0;
            do {
                if (curl_multi_perform(multi, &running) != CURLM_OK) {
                    code = CURLE_FAILED_INIT;
                    snprintf(errbuf, sizeof(errbuf), "curl_multi_perform failed");
                    break;
                }
                if (!running) break;
                if (options->cancelled && options->cancelled(options->context)) {
                    code = CURLE_ABORTED_BY_CALLBACK;
                    snprintf(errbuf, sizeof(errbuf), "sync cancelled");
                    break;
                }
                int numfds = 0;
                curl_multi_poll(multi, NULL, 0, 100, &numfds);
            } while (1);
            int queued = 0;
            CURLMsg *message;
            while ((message = curl_multi_info_read(multi, &queued))) {
                if (message->msg == CURLMSG_DONE) {
                    code = message->data.result;
                }
            }
            curl_multi_remove_handle(multi, easy);
        }
    }
    curl_off_t tcp_us = 0, tls_us = 0, first_byte_us = 0;
    curl_easy_getinfo(easy, CURLINFO_CONNECT_TIME_T, &tcp_us);
    curl_easy_getinfo(easy, CURLINFO_APPCONNECT_TIME_T, &tls_us);
    curl_easy_getinfo(easy, CURLINFO_STARTTRANSFER_TIME_T, &first_byte_us);
    out_resp->tcp_ms = tcp_us > 0 ? (uint64_t)tcp_us / 1000 : 0;
    out_resp->tls_ms = tls_us > tcp_us ? (uint64_t)(tls_us - tcp_us) / 1000 : 0;
    out_resp->first_byte_ms = first_byte_us > 0 ? (uint64_t)first_byte_us / 1000 : 0;
    if (rx_buf.timed_out) {
        code = CURLE_OPERATION_TIMEDOUT;
        snprintf(errbuf, sizeof(errbuf), "sync I/O timeout");
    }
    out_resp->curl_code = (int)code;

    long http_code = 0;
    curl_easy_getinfo(easy, CURLINFO_RESPONSE_CODE, &http_code);
    out_resp->http_status = (int)http_code;

    char *redirect_url = NULL;
    curl_easy_getinfo(easy, CURLINFO_REDIRECT_URL, &redirect_url);
    if (redirect_url) {
        snprintf(out_resp->redirect_url, sizeof(out_resp->redirect_url), "%s", redirect_url);
    }

    if (code != CURLE_OK) {
        snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "%s",
            errbuf[0] ? errbuf : curl_easy_strerror(code));
    }

    out_resp->response_body = rx_buf.data;
    out_resp->response_body_len = rx_buf.file ? 0 : rx_buf.size;
    out_resp->response_capacity_peak = rx_buf.capacity;
    out_resp->sampled_heap_peak = rx_buf.sampled_heap_peak;

    if (upload) fclose(upload);
    if (rx_buf.file) {
        int flush_result = fflush(rx_buf.file);
        int close_result = fclose(rx_buf.file);
        if (flush_result != 0 || close_result != 0) {
            if (code == CURLE_OK) {
                code = CURLE_WRITE_ERROR;
                out_resp->curl_code = code;
            }
        }
    }
    curl_slist_free_all(headers);
    curl_slist_free_all(resolve);
    if (code != CURLE_OK) {
        slot_teardown(slot);
    }
    return (code == CURLE_OK) ? 0 : (int)code;
}

int anki_3ds_https_post(
    int slot,
    const char *host,
    const char *path,
    const char *header_anki_sync,
    const uint8_t *body,
    size_t body_len,
    uint32_t timeout_ms,
    uint32_t connect_timeout_ms,
    const Anki3dsRequestOptions *options,
    Anki3dsHttpResponse *out_resp
) {
    if (!out_resp || !host || !path) return -1;
    memset(out_resp, 0, sizeof(*out_resp));
    if (!options) {
        snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "missing request options");
        return -1;
    }
    if (slot < 0 || slot >= HTTPS_SLOT_COUNT) slot = 0;
    if (options->cancelled && options->cancelled(options->context)) {
        snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "sync cancelled");
        return CURLE_ABORTED_BY_CALLBACK;
    }

    bool allow_override = false;
#ifdef ANKI_3DS_EMULATOR
    allow_override = true;
#endif
    uint64_t started = osGetTime();
    char ip[16] = {0};
    const char *cached = dns_cache_lookup(slot, host);
    int dns_rc = 0;
    if (cached) {
        snprintf(ip, sizeof(ip), "%s", cached);
    } else {
        char dns_error[160] = {0};
        dns_rc = probe_dns_with_context(host, ip, dns_error, dns_cancel, (void *)options, allow_override);
        if (dns_rc == 0) dns_cache_store(slot, host, ip);
    }
    out_resp->dns_ms = osGetTime() - started;
    if (dns_rc != 0) {
        out_resp->total_ms = out_resp->dns_ms;
        snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "DNS failed");
        return -2;
    }

    int result = 0;
    const uint32_t backoff_ms[] = {0, 250, 750, 1500};
    for (int attempt = 0; attempt < 4; attempt++) {
        if (attempt > 0) {
            if (options->upload_file == NULL && options->download_file != NULL) {
                /* truncate partial download file */
                FILE *f = fopen(options->download_file, "wb"); if (f) fclose(f);
            }
            if (options->write_body) {
                options->write_body(options->context, NULL, 0, 0);
            }
            anki_3ds_http_response_free(out_resp);
            memset(out_resp, 0, sizeof(*out_resp));
            slot_teardown(slot);
            dns_cache_forget(slot, host);
            for (uint32_t waited = 0; waited < backoff_ms[attempt]; waited += 50) {
                if (options->cancelled && options->cancelled(options->context)) {
                    snprintf(out_resp->error_msg, sizeof(out_resp->error_msg), "sync cancelled");
                    out_resp->total_ms = osGetTime() - started;
                    return CURLE_ABORTED_BY_CALLBACK;
                }
                svcSleepThread(50LL * 1000000LL);
            }
            char dns_error[160] = {0};
            if (probe_dns_with_context(host, ip, dns_error, dns_cancel, (void *)options, allow_override) != 0)
                continue;   /* keep last result, try again */
            dns_cache_store(slot, host, ip);
        }
        result = request_with_ip(slot, host, ip, path, header_anki_sync, body, body_len,
                                 timeout_ms, connect_timeout_ms, options, out_resp);
        if (result == 0 || !is_transient(result) || out_resp->http_status != 0) break;
    }
    out_resp->total_ms = osGetTime() - started;
    return result;
}

void anki_3ds_http_response_free(Anki3dsHttpResponse *resp) {
    if (resp && resp->response_body) {
        free(resp->response_body);
        resp->response_body = NULL;
        resp->response_body_len = 0;
    }
}

uint32_t anki_3ds_transport_heap_used(void) { return (uint32_t)mallinfo().uordblks; }
