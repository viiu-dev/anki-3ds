// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

#include "ui_renderer.h"
#include <3ds.h>
#include <3ds/services/mcuhwc.h>
#include <citro2d.h>
#include <citro3d.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define STB_TRUETYPE_IMPLEMENTATION
#include "stb_truetype.h"

#define STB_IMAGE_IMPLEMENTATION
#define STBI_NO_THREAD_LOCALS
#define STBI_ONLY_PNG
#define STBI_ONLY_JPEG
#define STBI_ONLY_BMP
#define STBI_ONLY_GIF
#include "stb_image.h"

#include "src/webp/decode.h"
#include "src/webp/demux.h"
#include "nanosvg.h"
#include "nanosvgrast.h"
#include <opus/opusfile.h>
#include <tremor/ivorbisfile.h>

#define MEDIA_IMAGE_SLOTS 8
#define MEDIA_PATH_MAX 512
#define MEDIA_IMAGE_MAX 256
#define MEDIA_FILE_MAX (8u * 1024u * 1024u)
#define MEDIA_PIXEL_MAX (4u * 1024u * 1024u)
#define MEDIA_SVG_MAX (2u * 1024u * 1024u)
#define MEDIA_MISSING_SLOTS 24

#define DR_MP3_IMPLEMENTATION
#define DR_MP3_NO_SIMD
#include "dr_mp3.h"

#define AUDIO_PCM_BYTES (1024 * 1024)
#define AUDIO_CHANNEL 0

// Glyphs are rasterized at their on-screen size. This atlas replaces the
// console's low-resolution system glyph sheet and can be refreshed without
// bundling a very large .bcfnt file.
#define CARD_ATLAS_W 1024
#define CARD_ATLAS_H 1024
#define CARD_GLYPH_CACHE_MAX 4096
// The existing layout was tuned against a roughly 30px system font.
#define CARD_FONT_EM_PX 30.0f
// Put the baseline inside the 19px/100% line box. Noto's CJK ink reaches
// about 0.84em above the baseline and 0.08em below it.
#define CARD_FONT_BASELINE 0.84f

typedef struct {
    uint32_t codepoint;
    int px;
    int glyph;
    int atlas_x;
    int atlas_y;
    int width;
    int height;
    int offset_x;
    int offset_y;
    float advance;
} CardGlyph;

static UiTheme g_theme = {
    C2D_RGBA(247, 247, 245, 255), C2D_RGBA(238, 241, 243, 255),
    C2D_RGBA(255, 255, 255, 255), C2D_RGBA(243, 244, 246, 255),
    C2D_RGBA(226, 229, 233, 255), C2D_RGBA(226, 227, 224, 255),
    C2D_RGBA(30, 32, 36, 255), C2D_RGBA(107, 114, 128, 255),
    C2D_RGBA(156, 163, 175, 255), C2D_RGBA(37, 99, 235, 255),
    C2D_RGBA(255, 255, 255, 255), C2D_RGBA(20, 100, 200, 255),
    C2D_RGBA(210, 60, 40, 255), C2D_RGBA(36, 130, 56, 255),
    C2D_RGBA(255, 243, 166, 255), C2D_RGBA(30, 32, 36, 255),
    C2D_RGBA(226, 238, 250, 255), C2D_RGBA(219, 234, 254, 255),
    C2D_RGBA(37, 99, 235, 255), C2D_RGBA(243, 244, 246, 255),
    C2D_RGBA(107, 114, 128, 255), C2D_RGBA(120, 126, 134, 36),
    C2D_RGBA(120, 126, 134, 130), C2D_RGBA(252, 232, 230, 255),
    C2D_RGBA(197, 40, 41, 255), C2D_RGBA(254, 243, 224, 255),
    C2D_RGBA(176, 110, 0, 255), C2D_RGBA(230, 244, 234, 255),
    C2D_RGBA(30, 142, 62, 255), C2D_RGBA(232, 240, 254, 255),
    C2D_RGBA(26, 115, 232, 255), C2D_RGBA(0, 0, 0, 26),
};

void ui_set_theme(const UiTheme* theme) {
    if (theme) g_theme = *theme;
}

static uint32_t mix_color(uint32_t a, uint32_t b, float t) {
    if (t < 0.0f) t = 0.0f;
    if (t > 1.0f) t = 1.0f;
    int ar = a & 0xFF, ag = (a >> 8) & 0xFF, ab = (a >> 16) & 0xFF, aa = (a >> 24) & 0xFF;
    int br = b & 0xFF, bg = (b >> 8) & 0xFF, bb = (b >> 16) & 0xFF, ba = (b >> 24) & 0xFF;
    int r = (int)(ar + (br - ar) * t);
    int g = (int)(ag + (bg - ag) * t);
    int bl = (int)(ab + (bb - ab) * t);
    int al = (int)(aa + (ba - aa) * t);
    return C2D_RGBA(r, g, bl, al);
}

static uint32_t darken(uint32_t color, float t) {
    return mix_color(color, C2D_RGBA(0, 0, 0, 255), t);
}

static C3D_RenderTarget* s_top_target = NULL;
static C3D_RenderTarget* s_bottom_target = NULL;
static C2D_TextBuf s_text_buf = NULL;
static C2D_TextBuf s_measure_buf = NULL;
static bool s_initialized = false;

static stbtt_fontinfo s_font;
static bool s_font_loaded = false;
static C3D_Tex s_card_atlas_tex;
static bool s_card_atlas_ready = false;
static uint8_t s_card_atlas[CARD_ATLAS_W * CARD_ATLAS_H * 4];
static CardGlyph s_glyphs[CARD_GLYPH_CACHE_MAX];
static int s_glyph_count = 0;
static int s_atlas_x = 0;
static int s_atlas_y = 0;
static int s_atlas_row_h = 0;
static bool s_atlas_dirty = false;
static bool s_atlas_full = false;
static uint8_t s_swizzle_map[64];
static bool s_swizzle_ready = false;

static void init_swizzle_map(void) {
    if (s_swizzle_ready) return;
    for (int i = 0; i < 64; i++) s_swizzle_map[i] = (uint8_t)i;
    static const uint8_t table[][4] = {
        {2, 8, 16, 4}, {3, 9, 17, 5}, {6, 10, 24, 20}, {7, 11, 25, 21},
        {14, 26, 28, 22}, {15, 27, 29, 23}, {34, 40, 48, 36}, {35, 41, 49, 37},
        {38, 42, 56, 52}, {39, 43, 57, 53}, {46, 58, 60, 54}, {47, 59, 61, 55},
    };
    for (size_t i = 0; i < sizeof(table) / sizeof(table[0]); i++) {
        uint8_t tmp = s_swizzle_map[table[i][0]];
        s_swizzle_map[table[i][0]] = s_swizzle_map[table[i][1]];
        s_swizzle_map[table[i][1]] = s_swizzle_map[table[i][2]];
        s_swizzle_map[table[i][2]] = s_swizzle_map[table[i][3]];
        s_swizzle_map[table[i][3]] = tmp;
    }
    uint8_t tmp = s_swizzle_map[12];
    s_swizzle_map[12] = s_swizzle_map[18];
    s_swizzle_map[18] = tmp;
    tmp = s_swizzle_map[13];
    s_swizzle_map[13] = s_swizzle_map[19];
    s_swizzle_map[19] = tmp;
    tmp = s_swizzle_map[44];
    s_swizzle_map[44] = s_swizzle_map[50];
    s_swizzle_map[50] = tmp;
    tmp = s_swizzle_map[45];
    s_swizzle_map[45] = s_swizzle_map[51];
    s_swizzle_map[51] = tmp;
    // The table above maps destination pixels to source pixels. Invert it
    // so swizzled_offset() can map source pixels to their final location.
    uint8_t source_to_dest[64];
    for (int dest = 0; dest < 64; dest++) {
        source_to_dest[s_swizzle_map[dest]] = (uint8_t)dest;
    }
    for (int i = 0; i < 64; i++) s_swizzle_map[i] = source_to_dest[i];
    s_swizzle_ready = true;
}

static size_t swizzled_offset_w(int x, int y, int width) {
    init_swizzle_map();
    int tile_x = x >> 3;
    int tile_y = y >> 3;
    int tiles_per_row = width >> 3;
    int tile_index = tile_y * tiles_per_row + tile_x;
    int pixel = (y & 7) * 8 + (x & 7);
    return ((size_t)tile_index * 64 + s_swizzle_map[pixel]) * 4;
}

static size_t swizzled_offset(int x, int y) {
    return swizzled_offset_w(x, y, CARD_ATLAS_W);
}

typedef struct {
    char path[MEDIA_PATH_MAX];
    C3D_Tex tex;
    bool ready;
    int image_w;
    int image_h;
    int tex_w;
    int tex_h;
    uint64_t used;
} MediaImage;

static MediaImage s_media_images[MEDIA_IMAGE_SLOTS];
static uint8_t s_media_scratch[MEDIA_IMAGE_MAX * MEDIA_IMAGE_MAX * 4];
static uint64_t s_media_clock;
static char s_media_missing[MEDIA_MISSING_SLOTS][MEDIA_PATH_MAX];
static uint32_t s_media_missing_next;

static bool media_missing(const char* path) {
    for (int i = 0; i < MEDIA_MISSING_SLOTS; i++) {
        if (s_media_missing[i][0] && strcmp(s_media_missing[i], path) == 0) return true;
    }
    return false;
}

static void media_mark_missing(const char* path) {
    if (strlen(path) >= sizeof(s_media_missing[0])) return;
    snprintf(s_media_missing[s_media_missing_next], sizeof(s_media_missing[0]), "%s", path);
    s_media_missing_next = (s_media_missing_next + 1) % MEDIA_MISSING_SLOTS;
}

void ui_media_prefetch_clear(void);

static bool s_capture_pixels_active = false;
static uint8_t* s_captured_pixels = NULL;
static int s_captured_w = 0;
static int s_captured_h = 0;

void ui_media_reset(void) {
    for (int i = 0; i < MEDIA_IMAGE_SLOTS; i++) {
        if (s_media_images[i].ready) {
            C3D_TexDelete(&s_media_images[i].tex);
            s_media_images[i].ready = false;
        }
        s_media_images[i].path[0] = '\0';
    }
    ui_media_prefetch_clear();
    free(s_captured_pixels);
    s_captured_pixels = NULL;
    for (int i = 0; i < MEDIA_MISSING_SLOTS; i++) s_media_missing[i][0] = '\0';
    s_media_missing_next = 0;
}

void ui_media_refresh(void) {
    for (int i = 0; i < MEDIA_MISSING_SLOTS; i++) s_media_missing[i][0] = '\0';
    s_media_missing_next = 0;
}

static int next_pow2(int value) {
    int result = 8;
    while (result < value && result < 1024) result <<= 1;
    return result;
}

static uint8_t* read_media_file(const char* path, size_t* out_size) {
    FILE* file = fopen(path, "rb");
    if (!file) return NULL;
    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return NULL;
    }
    long size = ftell(file);
    if (size <= 0 || (unsigned long)size > MEDIA_FILE_MAX) {
        fclose(file);
        return NULL;
    }
    rewind(file);
    uint8_t* data = (uint8_t*)malloc((size_t)size + 1);
    if (!data) {
        fclose(file);
        return NULL;
    }
    size_t got = fread(data, 1, (size_t)size, file);
    fclose(file);
    if (got != (size_t)size) {
        free(data);
        return NULL;
    }
    data[size] = 0;
    *out_size = (size_t)size;
    return data;
}

static bool bytes_are_webp(const uint8_t* data, size_t size) {
    return size >= 12 && memcmp(data, "RIFF", 4) == 0 && memcmp(data + 8, "WEBP", 4) == 0;
}

static bool bytes_contain(const uint8_t* data, size_t size, const char* needle) {
    size_t length = strlen(needle);
    if (length == 0 || size < length) return false;
    for (size_t i = 0; i + length <= size; i++) {
        if (memcmp(data + i, needle, length) == 0) return true;
    }
    return false;
}

static bool bytes_are_svg(const uint8_t* data, size_t size) {
    size_t start = 0;
    if (size >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF) start = 3;
    while (start < size && (data[start] == ' ' || data[start] == '\t'
        || data[start] == '\r' || data[start] == '\n')) {
        start++;
    }
    if (start + 4 <= size && strncasecmp((const char*)data + start, "<svg", 4) == 0) {
        return true;
    }
    size_t window = size < 8192 ? size : 8192;
    return bytes_contain(data, window, "<svg") || bytes_contain(data, window, "<SVG");
}

static void fit_dimensions(int src_w, int src_h, int* dst_w, int* dst_h) {
    int w = src_w;
    int h = src_h;
    if (w < 1) w = 1;
    if (h < 1) h = 1;
    if (w > MEDIA_IMAGE_MAX || h > MEDIA_IMAGE_MAX) {
        if (w >= h) {
            h = (int)((int64_t)h * MEDIA_IMAGE_MAX / w);
            w = MEDIA_IMAGE_MAX;
        } else {
            w = (int)((int64_t)w * MEDIA_IMAGE_MAX / h);
            h = MEDIA_IMAGE_MAX;
        }
    }
    if (w < 1) w = 1;
    if (h < 1) h = 1;
    *dst_w = w;
    *dst_h = h;
}

static void resample_rgba(
    const uint8_t* src,
    int src_w,
    int src_h,
    size_t src_stride,
    uint8_t* dst,
    int dst_w,
    int dst_h
) {
    for (int y = 0; y < dst_h; y++) {
        int sy = (int)((int64_t)y * src_h / dst_h);
        if (sy >= src_h) sy = src_h - 1;
        const uint8_t* src_row = src + (size_t)sy * src_stride;
        uint8_t* dst_row = dst + (size_t)y * (size_t)dst_w * 4;
        for (int x = 0; x < dst_w; x++) {
            int sx = (int)((int64_t)x * src_w / dst_w);
            if (sx >= src_w) sx = src_w - 1;
            const uint8_t* pixel = src_row + (size_t)sx * 4;
            dst_row[(size_t)x * 4 + 0] = pixel[0];
            dst_row[(size_t)x * 4 + 1] = pixel[1];
            dst_row[(size_t)x * 4 + 2] = pixel[2];
            dst_row[(size_t)x * 4 + 3] = pixel[3];
        }
    }
}

static bool upload_media_pixels(
    MediaImage* image,
    const uint8_t* pixels,
    int src_w,
    int src_h,
    size_t stride,
    int dst_w,
    int dst_h
) {
    if (!pixels || src_w <= 0 || src_h <= 0 || dst_w <= 0 || dst_h <= 0) return false;
    if (s_capture_pixels_active) {
        size_t bytes = (size_t)dst_w * (size_t)dst_h * 4;
        uint8_t* tight = (uint8_t*)malloc(bytes ? bytes : 1);
        if (!tight) return false;
        resample_rgba(pixels, src_w, src_h, stride, tight, dst_w, dst_h);
        free(s_captured_pixels);
        s_captured_pixels = tight;
        s_captured_w = dst_w;
        s_captured_h = dst_h;
        return true;
    }
    if (!image) return false;
    int tex_w = next_pow2(dst_w);
    int tex_h = next_pow2(dst_h);
    if (tex_w * tex_h * 4 > (int)sizeof(s_media_scratch)) return false;
    memset(s_media_scratch, 0, (size_t)tex_w * tex_h * 4);
    for (int y = 0; y < dst_h; y++) {
        int sy = (int)((int64_t)y * src_h / dst_h);
        if (sy >= src_h) sy = src_h - 1;
        for (int x = 0; x < dst_w; x++) {
            int sx = (int)((int64_t)x * src_w / dst_w);
            if (sx >= src_w) sx = src_w - 1;
            const uint8_t* src = pixels + (size_t)sy * stride + (size_t)sx * 4;
            uint8_t* dst = s_media_scratch + swizzled_offset_w(x, y, tex_w);
            dst[0] = src[3];
            dst[1] = src[2];
            dst[2] = src[1];
            dst[3] = src[0];
        }
    }
    if (!C3D_TexInit(&image->tex, (u16)tex_w, (u16)tex_h, GPU_RGBA8)) return false;
    C3D_TexSetFilter(&image->tex, GPU_LINEAR, GPU_LINEAR);
    C3D_TexSetWrap(&image->tex, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
    C3D_TexUpload(&image->tex, s_media_scratch);
    C3D_TexFlush(&image->tex);
    image->ready = true;
    image->image_w = dst_w;
    image->image_h = dst_h;
    image->tex_w = tex_w;
    image->tex_h = tex_h;
    return true;
}

static bool decode_webp_image(
    MediaImage* image,
    const uint8_t* data,
    size_t size,
    int* out_w,
    int* out_h
) {
    WebPDecoderConfig config;
    if (!WebPInitDecoderConfig(&config)) return false;
    if (WebPGetFeatures(data, size, &config.input) != VP8_STATUS_OK) return false;
    int src_w = config.input.width;
    int src_h = config.input.height;
    if (src_w <= 0 || src_h <= 0) return false;
    if ((uint64_t)src_w * (uint64_t)src_h > MEDIA_PIXEL_MAX) return false;
    int dst_w, dst_h;
    fit_dimensions(src_w, src_h, &dst_w, &dst_h);

    const uint8_t* frame = data;
    size_t frame_size = size;
    WebPDemuxer* demux = NULL;
    WebPIterator iterator;
    bool has_iterator = false;
    if (config.input.has_animation) {
        WebPData webp = { .bytes = data, .size = size };
        demux = WebPDemux(&webp);
        if (demux && WebPDemuxGetFrame(demux, 1, &iterator)) {
            frame = iterator.fragment.bytes;
            frame_size = iterator.fragment.size;
            has_iterator = true;
        }
    }

    config.options.use_scaling = 1;
    config.options.scaled_width = dst_w;
    config.options.scaled_height = dst_h;
    config.options.no_fancy_upsampling = 1;
    config.output.colorspace = MODE_RGBA;
    bool ok = WebPDecode(frame, frame_size, &config) == VP8_STATUS_OK;
    if (!ok) {
        WebPFreeDecBuffer(&config.output);
        config.options.use_scaling = 0;
        ok = WebPDecode(frame, frame_size, &config) == VP8_STATUS_OK;
    }
    if (ok) {
        ok = upload_media_pixels(
            image,
            config.output.u.RGBA.rgba,
            config.output.width,
            config.output.height,
            config.output.u.RGBA.stride,
            dst_w,
            dst_h);
    }
    WebPFreeDecBuffer(&config.output);
    if (has_iterator) WebPDemuxReleaseIterator(&iterator);
    if (demux) WebPDemuxDelete(demux);
    if (ok) {
        *out_w = dst_w;
        *out_h = dst_h;
    }
    return ok;
}

static bool decode_svg_image(
    MediaImage* image,
    char* text,
    size_t size,
    int* out_w,
    int* out_h
) {
    if (size > MEDIA_SVG_MAX) return false;
    NSVGimage* svg = nsvgParse(text, "px", 96.0f);
    if (!svg) return false;
    if (!isfinite(svg->width) || !isfinite(svg->height)
        || svg->width <= 0.0f || svg->height <= 0.0f
        || svg->width > 100000.0f || svg->height > 100000.0f) {
        nsvgDelete(svg);
        return false;
    }
    int shapes = 0;
    for (NSVGshape* shape = svg->shapes; shape; shape = shape->next) {
        if (++shapes > 2000) {
            nsvgDelete(svg);
            return false;
        }
    }
    int dst_w, dst_h;
    fit_dimensions((int)(svg->width + 0.5f), (int)(svg->height + 0.5f), &dst_w, &dst_h);
    float scale_x = (float)dst_w / svg->width;
    float scale_y = (float)dst_h / svg->height;
    float scale = scale_x < scale_y ? scale_x : scale_y;
    NSVGrasterizer* rasterizer = nsvgCreateRasterizer();
    if (!rasterizer) {
        nsvgDelete(svg);
        return false;
    }
    size_t bytes = (size_t)dst_w * (size_t)dst_h * 4;
    uint8_t* pixels = (uint8_t*)malloc(bytes);
    bool ok = false;
    if (pixels) {
        memset(pixels, 0, bytes);
        nsvgRasterize(rasterizer, svg, 0.0f, 0.0f, scale, pixels, dst_w, dst_h, dst_w * 4);
        ok = upload_media_pixels(image, pixels, dst_w, dst_h, (size_t)dst_w * 4, dst_w, dst_h);
        free(pixels);
    }
    nsvgDeleteRasterizer(rasterizer);
    nsvgDelete(svg);
    if (ok) {
        *out_w = dst_w;
        *out_h = dst_h;
    }
    return ok;
}

static bool decode_raster_image(
    MediaImage* image,
    const uint8_t* data,
    size_t size,
    int* out_w,
    int* out_h
) {
    int src_w = 0, src_h = 0, channels = 0;
    if (!stbi_info_from_memory(data, (int)size, &src_w, &src_h, &channels)) return false;
    if (src_w <= 0 || src_h <= 0) return false;
    if ((uint64_t)src_w * (uint64_t)src_h > MEDIA_PIXEL_MAX) return false;
    int dst_w, dst_h;
    fit_dimensions(src_w, src_h, &dst_w, &dst_h);
    unsigned char* pixels = stbi_load_from_memory(data, (int)size, &src_w, &src_h, &channels, 4);
    if (!pixels) return false;
    bool ok = upload_media_pixels(
        image,
        pixels,
        src_w,
        src_h,
        (size_t)src_w * 4,
        dst_w,
        dst_h);
    stbi_image_free(pixels);
    if (ok) {
        *out_w = dst_w;
        *out_h = dst_h;
    }
    return ok;
}

static bool decode_media_data(
    MediaImage* image,
    const uint8_t* data,
    size_t size,
    int* out_w,
    int* out_h
) {
    if (bytes_are_webp(data, size)) {
        return decode_webp_image(image, data, size, out_w, out_h);
    } else if (bytes_are_svg(data, size)) {
        return decode_svg_image(image, (char*)data, size, out_w, out_h);
    } else {
        return decode_raster_image(image, data, size, out_w, out_h);
    }
}

#define MEDIA_PREFETCH_SLOTS 4

typedef struct {
    char path[MEDIA_PATH_MAX];
    uint8_t* pixels;
    int width;
    int height;
    uint64_t used;
} MediaPrefetch;

static MediaPrefetch s_prefetch[MEDIA_PREFETCH_SLOTS];
static uint64_t s_prefetch_clock;

static int media_image_slot(void) {
    for (int i = 0; i < MEDIA_IMAGE_SLOTS; i++) {
        if (!s_media_images[i].ready) {
            return i;
        }
    }
    int slot = 0;
    for (int i = 1; i < MEDIA_IMAGE_SLOTS; i++) {
        if (s_media_images[i].used < s_media_images[slot].used) slot = i;
    }
    C3D_TexDelete(&s_media_images[slot].tex);
    s_media_images[slot].ready = false;
    return slot;
}

void ui_media_prefetch_clear(void) {
    for (int i = 0; i < MEDIA_PREFETCH_SLOTS; i++) {
        free(s_prefetch[i].pixels);
        s_prefetch[i].pixels = NULL;
        s_prefetch[i].path[0] = '\0';
        s_prefetch[i].width = 0;
        s_prefetch[i].height = 0;
        s_prefetch[i].used = 0;
    }
}

bool ui_media_prefetch(const char* path) {
    if (!path || !path[0]) return false;
    for (int i = 0; i < MEDIA_IMAGE_SLOTS; i++) {
        if (s_media_images[i].ready && strcmp(s_media_images[i].path, path) == 0) {
            return true;
        }
    }
    int hit = -1;
    for (int i = 0; i < MEDIA_PREFETCH_SLOTS; i++) {
        if (s_prefetch[i].pixels && strcmp(s_prefetch[i].path, path) == 0) {
            hit = i;
            break;
        }
    }
    if (hit >= 0) {
        s_prefetch[hit].used = ++s_prefetch_clock;
        return true;
    }
    size_t size = 0;
    uint8_t* data = read_media_file(path, &size);
    if (!data) return false;
    s_capture_pixels_active = true;
    free(s_captured_pixels);
    s_captured_pixels = NULL;
    s_captured_w = 0;
    s_captured_h = 0;
    int width = 0, height = 0;
    bool ok = decode_media_data(NULL, data, size, &width, &height);
    s_capture_pixels_active = false;
    free(data);
    if (!ok || !s_captured_pixels) {
        free(s_captured_pixels);
        s_captured_pixels = NULL;
        return false;
    }
    int slot = -1;
    for (int i = 0; i < MEDIA_PREFETCH_SLOTS; i++) {
        if (!s_prefetch[i].pixels) {
            slot = i;
            break;
        }
    }
    if (slot < 0) {
        slot = 0;
        for (int i = 1; i < MEDIA_PREFETCH_SLOTS; i++) {
            if (s_prefetch[i].used < s_prefetch[slot].used) slot = i;
        }
        free(s_prefetch[slot].pixels);
    }
    snprintf(s_prefetch[slot].path, sizeof(s_prefetch[slot].path), "%s", path);
    s_prefetch[slot].pixels = s_captured_pixels;
    s_prefetch[slot].width = s_captured_w;
    s_prefetch[slot].height = s_captured_h;
    s_prefetch[slot].used = ++s_prefetch_clock;
    s_captured_pixels = NULL;
    return true;
}

int ui_media_image(const char* path, int* out_width, int* out_height) {
    if (!path || !path[0]) return -1;
    for (int i = 0; i < MEDIA_IMAGE_SLOTS; i++) {
        if (s_media_images[i].ready && strcmp(s_media_images[i].path, path) == 0) {
            s_media_images[i].used = ++s_media_clock;
            if (out_width) *out_width = s_media_images[i].image_w;
            if (out_height) *out_height = s_media_images[i].image_h;
            return i;
        }
    }
    for (int i = 0; i < MEDIA_PREFETCH_SLOTS; i++) {
        if (s_prefetch[i].pixels && strcmp(s_prefetch[i].path, path) == 0) {
            int slot = media_image_slot();
            MediaImage* image = &s_media_images[slot];
            bool ok = upload_media_pixels(
                image,
                s_prefetch[i].pixels,
                s_prefetch[i].width,
                s_prefetch[i].height,
                (size_t)s_prefetch[i].width * 4,
                s_prefetch[i].width,
                s_prefetch[i].height);
            int width = s_prefetch[i].width;
            int height = s_prefetch[i].height;
            free(s_prefetch[i].pixels);
            s_prefetch[i].pixels = NULL;
            s_prefetch[i].path[0] = '\0';
            if (!ok) {
                if (image->ready) {
                    C3D_TexDelete(&image->tex);
                    image->ready = false;
                }
                media_mark_missing(path);
                return -1;
            }
            image->used = ++s_media_clock;
            snprintf(image->path, sizeof(image->path), "%s", path);
            if (out_width) *out_width = width;
            if (out_height) *out_height = height;
            return slot;
        }
    }
    if (media_missing(path)) return -1;
    int slot = media_image_slot();

    size_t size = 0;
    uint8_t* data = read_media_file(path, &size);
    if (!data) {
        media_mark_missing(path);
        return -1;
    }
    MediaImage* image = &s_media_images[slot];
    int width = 0, height = 0;
    bool ok = decode_media_data(image, data, size, &width, &height);
    free(data);
    if (!ok) {
        if (image->ready) {
            C3D_TexDelete(&image->tex);
            image->ready = false;
        }
        media_mark_missing(path);
        return -1;
    }
    image->used = ++s_media_clock;
    snprintf(image->path, sizeof(image->path), "%s", path);
    if (out_width) *out_width = width;
    if (out_height) *out_height = height;
    return slot;
}

void ui_media_draw(int id, float x, float y, float w, float h) {
    if (id < 0 || id >= MEDIA_IMAGE_SLOTS) return;
    MediaImage* image = &s_media_images[id];
    if (!image->ready || image->image_w <= 0 || image->image_h <= 0) return;
    if (w <= 0.0f || h <= 0.0f) return;
    Tex3DS_SubTexture subtexture = {
        .width = (u16)w,
        .height = (u16)h,
        .left = 0.0f,
        .top = 1.0f,
        .right = (float)image->image_w / (float)image->tex_w,
        .bottom = 1.0f - (float)image->image_h / (float)image->tex_h,
    };
    C2D_Image c2d = { .tex = &image->tex, .subtex = &subtexture };
    C2D_DrawImageAt(c2d, x, y, 0.55f, NULL, 1.0f, 1.0f);
}

static int16_t* s_audio_pcm = NULL;
static size_t s_audio_capacity = 0;
static int16_t* s_prefetch_pcm = NULL;
static char s_prefetch_audio_path[MEDIA_PATH_MAX];
static ndspWaveBuf s_audio_wave;
static bool s_audio_ready = false;

static uint16_t audio_le16(const uint8_t* data) {
    return (uint16_t)(data[0] | ((uint16_t)data[1] << 8));
}

static uint32_t audio_le32(const uint8_t* data) {
    return (uint32_t)data[0] | ((uint32_t)data[1] << 8)
        | ((uint32_t)data[2] << 16) | ((uint32_t)data[3] << 24);
}

static int16_t audio_clamp16(int32_t value) {
    if (value > 32767) return 32767;
    if (value < -32768) return -32768;
    return (int16_t)value;
}

static void audio_set_mix(int channels) {
    (void)channels;
    float mix[12] = {0.0f};
    mix[0] = 1.0f;
    mix[1] = 1.0f;
    ndspChnSetMix(AUDIO_CHANNEL, mix);
}

static bool audio_buffer_ensure(void) {
    if (s_audio_pcm) return true;
    size_t size = AUDIO_PCM_BYTES;
    while (size >= 128 * 1024) {
        s_audio_pcm = (int16_t*)linearAlloc(size);
        if (s_audio_pcm) {
            s_audio_capacity = size;
            break;
        }
        size /= 2;
    }
    if (!s_audio_pcm) {
        return false;
    }
    memset(s_audio_pcm, 0, s_audio_capacity);
    return true;
}

static bool audio_ndsp_ensure(void) {
    if (s_audio_ready) return true;
    if (R_FAILED(ndspInit())) {
        return false;
    }
    ndspSetOutputMode(NDSP_OUTPUT_STEREO);
    ndspChnReset(AUDIO_CHANNEL);
    ndspChnSetInterp(AUDIO_CHANNEL, NDSP_INTERP_LINEAR);
    ndspChnSetFormat(AUDIO_CHANNEL, NDSP_FORMAT_STEREO_PCM16);
    ndspChnSetRate(AUDIO_CHANNEL, 44100.0f);
    audio_set_mix(2);
    s_audio_ready = true;
    return true;
}

static bool audio_ensure(void) {
    return audio_buffer_ensure() && audio_ndsp_ensure();
}

void ui_audio_init(void) {
    audio_ensure();
}

void ui_audio_fini(void) {
    if (s_audio_pcm) {
        ndspChnWaveBufClear(AUDIO_CHANNEL);
        ndspChnSetPaused(AUDIO_CHANNEL, true);
        linearFree(s_audio_pcm);
        s_audio_pcm = NULL;
        s_audio_capacity = 0;
    }
    free(s_prefetch_pcm);
    s_prefetch_pcm = NULL;
    s_prefetch_audio_path[0] = '\0';
    if (s_audio_ready) ndspExit();
    s_audio_ready = false;
}

void ui_audio_stop(void) {
    if (!s_audio_ready) return;
    ndspChnWaveBufClear(AUDIO_CHANNEL);
    ndspChnSetPaused(AUDIO_CHANNEL, true);
}

static bool audio_start(int channels, int rate, size_t samples) {
    if (channels < 1 || channels > 2 || rate < 4000 || rate > 192000 || samples == 0) {
        return false;
    }
    if (samples % (size_t)channels != 0) return false;
    size_t frames = samples / (size_t)channels;
    if (frames == 0) return false;
    ui_audio_stop();
    memset(&s_audio_wave, 0, sizeof(s_audio_wave));
    s_audio_wave.data_vaddr = s_audio_pcm;
    s_audio_wave.nsamples = (u32)frames;
    s_audio_wave.looping = false;
    ndspChnSetFormat(
        AUDIO_CHANNEL,
        channels == 1 ? NDSP_FORMAT_MONO_PCM16 : NDSP_FORMAT_STEREO_PCM16);
    ndspChnSetRate(AUDIO_CHANNEL, (float)rate);
    audio_set_mix(channels);
    DSP_FlushDataCache(s_audio_pcm, (u32)(samples * sizeof(int16_t)));
    ndspChnWaveBufAdd(AUDIO_CHANNEL, &s_audio_wave);
    ndspChnSetPaused(AUDIO_CHANNEL, false);
    return true;
}

static bool audio_decode_mp3(const char* path, int* out_channels, int* out_rate, size_t* out_samples) {
    drmp3 mp3;
    if (!drmp3_init_file(&mp3, path, NULL)) return false;
    int channels = mp3.channels;
    int rate = (int)mp3.sampleRate;
    if (channels < 1 || channels > 2) {
        drmp3_uninit(&mp3);
        return false;
    }
    size_t capacity = s_audio_capacity / sizeof(int16_t);
    size_t total = 0;
    while (total < capacity) {
        drmp3_uint64 want = (capacity - total) / (size_t)channels;
        if (want == 0) break;
        drmp3_uint64 got = drmp3_read_pcm_frames_s16(&mp3, want, s_audio_pcm + total);
        if (got == 0) break;
        total += (size_t)got * (size_t)channels;
    }
    drmp3_uninit(&mp3);
    if (total == 0) return false;
    *out_channels = channels;
    *out_rate = rate;
    *out_samples = total;
    return true;
}

static bool audio_decode_vorbis(const char* path, int* out_channels, int* out_rate, size_t* out_samples) {
    FILE* source = fopen(path, "rb");
    if (!source) return false;
    OggVorbis_File file;
    if (ov_open(source, &file, NULL, 0) != 0) {
        fclose(source);
        return false;
    }
    vorbis_info* info = ov_info(&file, -1);
    if (!info || info->channels < 1) {
        ov_clear(&file);
        return false;
    }
    int channels = info->channels > 2 ? 2 : info->channels;
    int rate = (int)info->rate;
    size_t capacity = s_audio_capacity / sizeof(int16_t);
    size_t total = 0;
    int16_t temp[4096];
    bool ok = false;
    for (;;) {
        int bitstream = 0;
        long bytes = ov_read(&file, (char*)temp, (int)sizeof(temp), &bitstream);
        if (bytes == 0) {
            ok = true;
            break;
        }
        if (bytes < 0) {
            if (bytes == OV_HOLE) continue;
            break;
        }
        size_t values = (size_t)bytes / sizeof(int16_t);
        size_t frames = values / (size_t)info->channels;
        for (size_t frame = 0; frame < frames && total + (size_t)channels <= capacity; frame++) {
            for (int channel = 0; channel < channels; channel++) {
                s_audio_pcm[total++] = temp[frame * (size_t)info->channels + (size_t)channel];
            }
        }
        if (total + (size_t)channels > capacity) {
            ok = true;
            break;
        }
    }
    ov_clear(&file);
    if (!ok || total == 0) return false;
    *out_channels = channels;
    *out_rate = rate;
    *out_samples = total;
    return true;
}

static bool audio_decode_opus(const char* path, int* out_channels, int* out_rate, size_t* out_samples) {
    int error = 0;
    OggOpusFile* file = op_open_file(path, &error);
    if (!file) return false;
    size_t capacity = s_audio_capacity / sizeof(int16_t);
    size_t total = 0;
    bool ok = false;
    for (;;) {
        int frames = op_read_stereo(file, s_audio_pcm + total, (int)(capacity - total));
        if (frames == 0) {
            ok = true;
            break;
        }
        if (frames < 0) break;
        total += (size_t)frames * 2;
        if (total + 2 > capacity) {
            ok = true;
            break;
        }
    }
    op_free(file);
    if (!ok || total == 0) return false;
    *out_channels = 2;
    *out_rate = 48000;
    *out_samples = total;
    return true;
}

typedef struct {
    uint16_t format;
    uint16_t channels;
    uint32_t rate;
    uint16_t block_align;
    uint16_t bits;
    const uint8_t* data;
    size_t data_size;
} WavInfo;

static bool audio_parse_wav(const uint8_t* data, size_t size, WavInfo* wav) {
    if (size < 12 || memcmp(data, "RIFF", 4) != 0 || memcmp(data + 8, "WAVE", 4) != 0) {
        return false;
    }
    memset(wav, 0, sizeof(*wav));
    bool have_format = false;
    size_t pos = 12;
    while (pos + 8 <= size) {
        const uint8_t* chunk = data + pos;
        uint32_t length = audio_le32(chunk + 4);
        pos += 8;
        if (length > size - pos) length = (uint32_t)(size - pos);
        if (memcmp(chunk, "fmt ", 4) == 0 && length >= 16) {
            wav->format = audio_le16(chunk + 8);
            wav->channels = audio_le16(chunk + 10);
            wav->rate = audio_le32(chunk + 12);
            wav->block_align = audio_le16(chunk + 20);
            wav->bits = audio_le16(chunk + 22);
            if (wav->format == 0xFFFE && length >= 40) wav->format = audio_le16(chunk + 32);
            have_format = true;
        } else if (memcmp(chunk, "data", 4) == 0) {
            wav->data = chunk + 8;
            wav->data_size = length;
            if (have_format) break;
        }
        pos += length + (length & 1);
    }
    return have_format && wav->data && wav->channels > 0 && wav->rate > 0
        && (wav->format == 1 || wav->format == 3);
}

static bool audio_decode_wav(const char* path, int* out_channels, int* out_rate, size_t* out_samples) {
    size_t size = 0;
    uint8_t* data = read_media_file(path, &size);
    if (!data) return false;
    WavInfo wav;
    bool parsed = audio_parse_wav(data, size, &wav);
    if (!parsed) {
        free(data);
        return false;
    }
    int source_channels = wav.channels;
    int channels = source_channels > 2 ? 2 : source_channels;
    int bytes_per_sample = wav.bits / 8;
    int frame_size = wav.block_align ? wav.block_align : source_channels * bytes_per_sample;
    if (bytes_per_sample < 1 || bytes_per_sample > 4 || frame_size < source_channels * bytes_per_sample) {
        free(data);
        return false;
    }
    size_t capacity = s_audio_capacity / sizeof(int16_t);
    size_t total = 0;
    size_t frames = wav.data_size / (size_t)frame_size;
    for (size_t frame = 0; frame < frames && total + (size_t)channels <= capacity; frame++) {
        const uint8_t* source = wav.data + frame * (size_t)frame_size;
        for (int channel = 0; channel < channels; channel++) {
            const uint8_t* sample = source + (size_t)channel * (size_t)bytes_per_sample;
            int32_t value = 0;
            if (wav.format == 3 && wav.bits == 32) {
                float decoded = 0.0f;
                memcpy(&decoded, sample, sizeof(decoded));
                if (decoded > 1.0f) decoded = 1.0f;
                if (decoded < -1.0f) decoded = -1.0f;
                value = (int32_t)(decoded * 32767.0f);
            } else if (wav.bits == 8) {
                value = ((int32_t)sample[0] - 128) * 256;
            } else if (wav.bits == 16) {
                value = (int16_t)audio_le16(sample);
            } else if (wav.bits == 24) {
                value = (int32_t)((uint32_t)sample[0] << 8 | (uint32_t)sample[1] << 16 | (uint32_t)sample[2] << 24) >> 16;
            } else if (wav.bits == 32) {
                value = (int32_t)audio_le32(sample) >> 16;
            } else {
                free(data);
                return false;
            }
            s_audio_pcm[total++] = audio_clamp16(value);
        }
    }
    free(data);
    if (total == 0) return false;
    *out_channels = channels;
    *out_rate = (int)wav.rate;
    *out_samples = total;
    return true;
}

static bool audio_extension_is(const char* path, const char* first, const char* second) {
    const char* dot = strrchr(path, '.');
    if (!dot) return false;
    dot++;
    return strcasecmp(dot, first) == 0 || (second && strcasecmp(dot, second) == 0);
}

static bool audio_decode_path(const char* path, int* out_channels, int* out_rate, size_t* out_samples) {
    if (audio_extension_is(path, "ogg", "oga")) {
        return audio_decode_vorbis(path, out_channels, out_rate, out_samples);
    } else if (audio_extension_is(path, "opus", "spx")) {
        return audio_decode_opus(path, out_channels, out_rate, out_samples);
    } else if (audio_extension_is(path, "wav", "wave")) {
        return audio_decode_wav(path, out_channels, out_rate, out_samples);
    } else {
        return audio_decode_mp3(path, out_channels, out_rate, out_samples);
    }
}

static int s_prefetch_channels = 0;
static int s_prefetch_rate = 0;
static size_t s_prefetch_samples = 0;

void ui_audio_prefetch_clear(void) {
    free(s_prefetch_pcm);
    s_prefetch_pcm = NULL;
    s_prefetch_channels = 0;
    s_prefetch_rate = 0;
    s_prefetch_samples = 0;
    s_prefetch_audio_path[0] = '\0';
}

bool ui_audio_prefetch(const char* path) {
    if (!path || !path[0]) return false;
    if (s_prefetch_pcm && strcmp(s_prefetch_audio_path, path) == 0) return true;
    if (!audio_buffer_ensure()) return false;
    int16_t* saved_pcm = s_audio_pcm;
    size_t saved_capacity = s_audio_capacity;
    int16_t* buffer = (int16_t*)malloc(AUDIO_PCM_BYTES);
    if (!buffer) return false;
    s_audio_pcm = buffer;
    s_audio_capacity = AUDIO_PCM_BYTES;
    int channels = 0;
    int rate = 0;
    size_t samples = 0;
    bool ok = audio_decode_path(path, &channels, &rate, &samples);
    s_audio_pcm = saved_pcm;
    s_audio_capacity = saved_capacity;
    if (!ok || samples == 0) {
        free(buffer);
        return false;
    }
    free(s_prefetch_pcm);
    s_prefetch_pcm = buffer;
    s_prefetch_channels = channels;
    s_prefetch_rate = rate;
    s_prefetch_samples = samples;
    snprintf(s_prefetch_audio_path, sizeof(s_prefetch_audio_path), "%s", path);
    return true;
}

bool ui_audio_is_playing(void) {
    return s_audio_ready && ndspChnIsPlaying(AUDIO_CHANNEL);
}

bool ui_audio_decode(const char* path) {
    if (!path || !path[0]) return false;
    if (!audio_buffer_ensure()) return false;
    int channels = 0;
    int rate = 0;
    size_t samples = 0;
    return audio_decode_path(path, &channels, &rate, &samples);
}

bool ui_audio_play(const char* path) {
    if (!path || !path[0]) return false;
    if (!audio_buffer_ensure()) return false;
    if (s_prefetch_pcm && strcmp(s_prefetch_audio_path, path) == 0
        && s_prefetch_samples > 0
        && s_prefetch_samples * sizeof(int16_t) <= s_audio_capacity
        && audio_ndsp_ensure()) {
        memcpy(s_audio_pcm, s_prefetch_pcm, s_prefetch_samples * sizeof(int16_t));
        if (audio_start(s_prefetch_channels, s_prefetch_rate, s_prefetch_samples)) {
            return true;
        }
    }
    int channels = 0;
    int rate = 0;
    size_t samples = 0;
    bool ok = audio_decode_path(path, &channels, &rate, &samples);
    if (!ok) {
        if (audio_extension_is(path, "mp3", NULL)) {
            ok = audio_decode_vorbis(path, &channels, &rate, &samples)
                || audio_decode_opus(path, &channels, &rate, &samples);
        } else if (!audio_extension_is(path, "ogg", "oga") && !audio_extension_is(path, "opus", "spx")
            && !audio_extension_is(path, "wav", "wave")) {
            ok = audio_decode_vorbis(path, &channels, &rate, &samples)
                || audio_decode_opus(path, &channels, &rate, &samples)
                || audio_decode_wav(path, &channels, &rate, &samples);
        }
    }
    if (!ok) return false;
    if (!audio_ndsp_ensure()) return false;
    return audio_start(channels, rate, samples);
}

static int card_font_px(float scale) {
    int px = (int)lroundf(CARD_FONT_EM_PX * scale);
    return px < 6 ? 6 : px;
}

static void reset_card_atlas(void) {
    memset(s_card_atlas, 0, sizeof(s_card_atlas));
    s_glyph_count = 0;
    s_atlas_x = 0;
    s_atlas_y = 0;
    s_atlas_row_h = 0;
    s_atlas_dirty = true;
    s_atlas_full = false;
}

static CardGlyph* find_glyph(uint32_t codepoint, int px) {
    for (int i = 0; i < s_glyph_count; i++) {
        if (s_glyphs[i].codepoint == codepoint && s_glyphs[i].px == px) {
            return &s_glyphs[i];
        }
    }
    return NULL;
}

static uint32_t utf8_next(const unsigned char** cursor) {
    const unsigned char* p = *cursor;
    uint32_t cp = *p++;
    if (cp < 0x80) {
        *cursor = p;
        return cp;
    }
    if ((cp & 0xE0) == 0xC0 && (p[0] & 0xC0) == 0x80) {
        cp = ((cp & 0x1F) << 6) | (p[0] & 0x3F);
        *cursor = p + 1;
        return cp;
    }
    if ((cp & 0xF0) == 0xE0 && (p[0] & 0xC0) == 0x80 && (p[1] & 0xC0) == 0x80) {
        cp = ((cp & 0x0F) << 12) | ((p[0] & 0x3F) << 6) | (p[1] & 0x3F);
        *cursor = p + 2;
        return cp;
    }
    if ((cp & 0xF8) == 0xF0 && (p[0] & 0xC0) == 0x80 && (p[1] & 0xC0) == 0x80 &&
        (p[2] & 0xC0) == 0x80) {
        cp = ((cp & 0x07) << 18) | ((p[0] & 0x3F) << 12) |
             ((p[1] & 0x3F) << 6) | (p[2] & 0x3F);
        *cursor = p + 3;
        return cp;
    }
    *cursor = p;
    return 0xFFFD;
}

static CardGlyph* ensure_glyph(uint32_t codepoint, int px) {
    CardGlyph* cached = find_glyph(codepoint, px);
    if (cached) {
        return cached;
    }
    if (s_glyph_count >= CARD_GLYPH_CACHE_MAX || s_atlas_full) {
        s_atlas_full = true;
        return NULL;
    }

    float em_scale = stbtt_ScaleForMappingEmToPixels(&s_font, (float)px);
    int glyph = stbtt_FindGlyphIndex(&s_font, (int)codepoint);
    int advance_units = 0;
    int lsb = 0;
    stbtt_GetGlyphHMetrics(&s_font, glyph, &advance_units, &lsb);
    (void)lsb;

    int width = 0;
    int height = 0;
    int offset_x = 0;
    int offset_y = 0;
    unsigned char* bitmap =
        stbtt_GetGlyphBitmap(&s_font, em_scale, em_scale, glyph, &width, &height, &offset_x, &offset_y);

    int cell_w = width + 2;
    int cell_h = height + 2;
    if (cell_w < 2) cell_w = 2;
    if (cell_h < 2) cell_h = 2;
    if (s_atlas_x + cell_w > CARD_ATLAS_W) {
        s_atlas_x = 0;
        s_atlas_y += s_atlas_row_h + 1;
        s_atlas_row_h = 0;
    }
    if (s_atlas_y + cell_h > CARD_ATLAS_H) {
        s_atlas_full = true;
        if (bitmap) stbtt_FreeBitmap(bitmap, NULL);
        return NULL;
    }

    CardGlyph* glyph_entry = &s_glyphs[s_glyph_count++];
    glyph_entry->codepoint = codepoint;
    glyph_entry->px = px;
    glyph_entry->glyph = glyph;
    glyph_entry->atlas_x = s_atlas_x + 1;
    glyph_entry->atlas_y = s_atlas_y + 1;
    glyph_entry->width = width;
    glyph_entry->height = height;
    glyph_entry->offset_x = offset_x;
    glyph_entry->offset_y = offset_y;
    glyph_entry->advance = advance_units * em_scale;

    if (bitmap && width > 0 && height > 0) {
        for (int row = 0; row < height; row++) {
            const unsigned char* src = bitmap + (size_t)row * width;
            for (int col = 0; col < width; col++) {
                uint8_t alpha = src[col];
                uint8_t* dst = s_card_atlas + swizzled_offset(
                    glyph_entry->atlas_x + col, glyph_entry->atlas_y + row);
                // 3DS RGBA8 textures are stored in ABGR byte order.
                dst[0] = alpha;
                dst[1] = 255;
                dst[2] = 255;
                dst[3] = 255;
            }
        }
        stbtt_FreeBitmap(bitmap, NULL);
    } else if (bitmap) {
        stbtt_FreeBitmap(bitmap, NULL);
    }

    if (cell_h > s_atlas_row_h) {
        s_atlas_row_h = cell_h;
    }
    s_atlas_x += cell_w + 1;
    s_atlas_dirty = true;
    return glyph_entry;
}

static void commit_card_atlas(void) {
    if (!s_card_atlas_ready || !s_atlas_dirty) return;
    C3D_TexUpload(&s_card_atlas_tex, s_card_atlas);
    C3D_TexFlush(&s_card_atlas_tex);
    s_atlas_dirty = false;
}

void ui_prepare_card_text(float scale, const char* str) {
    (void)ui_prepare_card_text_checked(scale, str);
}

bool ui_prepare_card_text_checked(float scale, const char* str) {
    if (!s_font_loaded || !str) return true;
    int px = card_font_px(scale);
    const unsigned char* cursor = (const unsigned char*)str;
    while (*cursor) {
        uint32_t cp = utf8_next(&cursor);
        if (!ensure_glyph(cp, px)) {
            return false;
        }
    }
    return true;
}

void ui_commit_card_text(void) {
    commit_card_atlas();
}

void ui_reset_card_text(void) {
    reset_card_atlas();
}

bool ui_init(const void* font_data, size_t font_size) {
    if (s_initialized) return true;

    osSetSpeedupEnable(true);
    if (!C3D_Init(C3D_DEFAULT_CMDBUF_SIZE)) {
        return false;
    }
    if (!C2D_Init(C2D_DEFAULT_MAX_OBJECTS)) {
        C3D_Fini();
        return false;
    }
    C2D_Prepare();

    s_top_target = C2D_CreateScreenTarget(GFX_TOP, GFX_LEFT);
    s_bottom_target = C2D_CreateScreenTarget(GFX_BOTTOM, GFX_LEFT);
    // These counts are glyphs, not bytes. Full-screen cards only draw the
    // visible lines, so 8192 leaves ample headroom without reserving a large
    // block of the Old 3DS linear heap.
    s_text_buf = C2D_TextBufNew(8192);
    s_measure_buf = C2D_TextBufNew(2048);

    int font_offset = -1;
    if (font_data && font_size > 0) {
        font_offset = stbtt_GetFontOffsetForIndex((const unsigned char*)font_data, 0);
    }
    s_font_loaded = font_offset >= 0 &&
        stbtt_InitFont(&s_font, (const unsigned char*)font_data, font_offset);
    if (s_font_loaded) {
        s_card_atlas_ready = C3D_TexInit(&s_card_atlas_tex, CARD_ATLAS_W, CARD_ATLAS_H, GPU_RGBA8);
        if (s_card_atlas_ready) {
            C3D_TexSetFilter(&s_card_atlas_tex, GPU_NEAREST, GPU_NEAREST);
            C3D_TexSetWrap(&s_card_atlas_tex, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
            reset_card_atlas();
            commit_card_atlas();
        } else {
            s_font_loaded = false;
        }
    }

    ui_audio_init();

    s_initialized = true;
    return true;
}

void ui_fini(void) {
    if (!s_initialized) return;

    ui_audio_fini();
    ui_media_reset();

    if (s_text_buf) {
        C2D_TextBufDelete(s_text_buf);
        s_text_buf = NULL;
    }
    if (s_measure_buf) {
        C2D_TextBufDelete(s_measure_buf);
        s_measure_buf = NULL;
    }
    if (s_card_atlas_ready) {
        C3D_TexDelete(&s_card_atlas_tex);
        s_card_atlas_ready = false;
    }
    s_font_loaded = false;
    s_glyph_count = 0;
    s_atlas_dirty = false;
    s_atlas_full = false;

    C2D_Fini();
    C3D_Fini();
    s_initialized = false;
}

void ui_frame_begin(void) {
    C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
    C3D_DepthTest(false, GPU_ALWAYS, GPU_WRITE_ALL);
    commit_card_atlas();
    if (s_text_buf) {
        C2D_TextBufClear(s_text_buf);
    }
}

void ui_frame_end(void) {
    C3D_FrameEnd(0);
}

void ui_target_top(uint32_t bg_color) {
    if (!s_top_target) return;
    C2D_TargetClear(s_top_target, bg_color);
    C2D_SceneBegin(s_top_target);
}

void ui_target_bottom(uint32_t bg_color) {
    if (!s_bottom_target) return;
    C2D_TargetClear(s_bottom_target, bg_color);
    C2D_SceneBegin(s_bottom_target);
}

void ui_draw_rect(float x, float y, float w, float h, uint32_t color) {
    x = roundf(x);
    y = roundf(y);
    w = roundf(w);
    h = roundf(h);
    C2D_DrawRectSolid(x, y, 0.5f, w, h, color);
}

void ui_draw_rounded_rect(float x, float y, float w, float h, float radius, uint32_t color) {
    if (w <= 0.0f || h <= 0.0f) return;
    if (radius > w * 0.5f) radius = w * 0.5f;
    if (radius > h * 0.5f) radius = h * 0.5f;
    if (radius < 0.0f) radius = 0.0f;
    if (radius <= 0.5f) {
        ui_draw_rect(x, y, w, h, color);
        return;
    }
    ui_draw_rect(x + radius, y, w - radius * 2.0f, h, color);
    ui_draw_rect(x, y + radius, radius, h - radius * 2.0f, color);
    ui_draw_rect(x + w - radius, y + radius, radius, h - radius * 2.0f, color);
    ui_draw_circle(x + radius, y + radius, radius, color);
    ui_draw_circle(x + w - radius, y + radius, radius, color);
    ui_draw_circle(x + radius, y + h - radius, radius, color);
    ui_draw_circle(x + w - radius, y + h - radius, radius, color);
}

void ui_draw_rect_gradient(float x, float y, float w, float h, uint32_t c_top, uint32_t c_bot) {
    C2D_DrawRectangle(x, y, 0.5f, w, h, c_top, c_top, c_bot, c_bot);
}

void ui_draw_line(float x0, float y0, float x1, float y1, float thickness, uint32_t color) {
    C2D_DrawLine(roundf(x0), roundf(y0), color, roundf(x1), roundf(y1), color, thickness, 0.5f);
}

void ui_draw_circle(float x, float y, float radius, uint32_t color) {
    C2D_DrawCircleSolid(x, y, 0.5f, radius, color);
}

void ui_draw_triangle(float x0, float y0, float x1, float y1, float x2, float y2, uint32_t color) {
    C2D_DrawTriangle(x0, y0, color, x1, y1, color, x2, y2, color, 0.5f);
}

static bool draw_glyph_run(float x, float y, float scale, uint32_t color,
                           const char* str, bool bold, bool italic);

void ui_draw_text(float x, float y, float scale, uint32_t color, const char* str) {
    if (!str || !str[0]) return;
    if (s_font_loaded && s_card_atlas_ready &&
        draw_glyph_run(x, y, scale, color, str, false, false)) {
        return;
    }
    if (!s_text_buf) return;
    C2D_Text text;
    C2D_TextParse(&text, s_text_buf, str);
    C2D_TextOptimize(&text);
    C2D_DrawText(&text, C2D_WithColor, roundf(x), roundf(y), 0.6f, scale, scale, color);
}

float ui_get_text_width(float scale, const char* str) {
    if (!str || !str[0]) return 0.0f;
    if (s_font_loaded) {
        return ui_get_card_text_width(scale, str);
    }
    if (!s_measure_buf) return 0.0f;
    C2D_TextBufClear(s_measure_buf);
    C2D_Text text;
    C2D_TextParse(&text, s_measure_buf, str);
    float w = 0.0f, h = 0.0f;
    C2D_TextGetDimensions(&text, scale, scale, &w, &h);
    return w;
}

static void draw_system_card_text(float x, float y, float scale, uint32_t color, const char* str, bool bold, bool italic) {
    if (!str || !str[0] || !s_text_buf) return;
    C2D_Text text;
    C2D_TextParse(&text, s_text_buf, str);
    C2D_TextOptimize(&text);
    x = roundf(x);
    y = roundf(y);
    C2D_DrawText(&text, C2D_WithColor, x, y, 0.6f, scale, scale, color);
    if (bold) {
        C2D_DrawText(&text, C2D_WithColor, x + 1.0f, y, 0.6f, scale, scale, color);
    }
    if (italic) {
        C2D_DrawText(&text, C2D_WithColor, x + 0.9f, y + 0.4f, 0.6f, scale, scale,
                     (color & 0x00ffffff) | 0xb0000000);
    }
}

static void draw_card_glyph(const CardGlyph* glyph, float x, float y, uint32_t color) {
    if (!glyph || glyph->width <= 0 || glyph->height <= 0 || !s_card_atlas_ready) return;
    Tex3DS_SubTexture subtexture = {
        .width = (u16)glyph->width,
        .height = (u16)glyph->height,
        .left = (float)glyph->atlas_x / (float)CARD_ATLAS_W,
        .top = 1.0f - (float)glyph->atlas_y / (float)CARD_ATLAS_H,
        .right = (float)(glyph->atlas_x + glyph->width) / (float)CARD_ATLAS_W,
        .bottom = 1.0f - (float)(glyph->atlas_y + glyph->height) / (float)CARD_ATLAS_H,
    };
    C2D_Image image = { .tex = &s_card_atlas_tex, .subtex = &subtexture };
    C2D_ImageTint tint;
    C2D_PlainImageTint(&tint, color, 1.0f);
    float px = roundf(x + (float)glyph->offset_x);
    float py = roundf(y + (float)glyph->offset_y);
    C2D_DrawImageAt(image, px, py, 0.6f, &tint, 1.0f, 1.0f);
}

static bool draw_glyph_run(float x, float y, float scale, uint32_t color,
                           const char* str, bool bold, bool italic) {
    int px = card_font_px(scale);
    const unsigned char* cursor = (const unsigned char*)str;
    while (*cursor) {
        uint32_t cp = utf8_next(&cursor);
        if (!ensure_glyph(cp, px)) {
            return false;
        }
    }
    float em_scale = stbtt_ScaleForMappingEmToPixels(&s_font, (float)px);
    float baseline = y + CARD_FONT_BASELINE * (float)px;
    float pen_x = x;
    int previous_glyph = -1;
    cursor = (const unsigned char*)str;
    while (*cursor) {
        uint32_t cp = utf8_next(&cursor);
        int glyph_index = stbtt_FindGlyphIndex(&s_font, (int)cp);
        if (previous_glyph >= 0) {
            pen_x += stbtt_GetGlyphKernAdvance(&s_font, previous_glyph, glyph_index) * em_scale;
        }
        CardGlyph* glyph = find_glyph(cp, px);
        if (glyph) {
            draw_card_glyph(glyph, pen_x, baseline, color);
            if (bold) {
                draw_card_glyph(glyph, pen_x + 0.7f, baseline, color);
            }
            if (italic) {
                draw_card_glyph(glyph, pen_x + 0.9f, baseline + 0.4f,
                                (color & 0x00ffffff) | 0xb0000000);
            }
            pen_x += glyph->advance;
        }
        previous_glyph = glyph_index;
    }
    return true;
}

void ui_draw_card_text(float x, float y, float scale, uint32_t color, const char* str, bool bold, bool italic) {
    if (!str || !str[0]) return;
    if (!s_font_loaded || !s_card_atlas_ready ||
        !draw_glyph_run(x, y, scale, color, str, bold, italic)) {
        draw_system_card_text(x, y, scale, color, str, bold, italic);
        return;
    }
    commit_card_atlas();
}

float ui_get_card_text_width(float scale, const char* str) {
    if (!str || !str[0]) return 0.0f;
    if (!s_font_loaded) {
        if (!s_measure_buf) return 0.0f;
        C2D_TextBufClear(s_measure_buf);
        C2D_Text text;
        C2D_TextParse(&text, s_measure_buf, str);
        float w = 0.0f, h = 0.0f;
        C2D_TextGetDimensions(&text, scale, scale, &w, &h);
        return w;
    }

    int px = card_font_px(scale);
    float em_scale = stbtt_ScaleForMappingEmToPixels(&s_font, (float)px);
    float width = 0.0f;
    int previous_glyph = -1;
    const unsigned char* cursor = (const unsigned char*)str;
    while (*cursor) {
        uint32_t cp = utf8_next(&cursor);
        int glyph_index = stbtt_FindGlyphIndex(&s_font, (int)cp);
        if (previous_glyph >= 0) {
            width += stbtt_GetGlyphKernAdvance(&s_font, previous_glyph, glyph_index) * em_scale;
        }
        int advance = 0;
        int lsb = 0;
        stbtt_GetGlyphHMetrics(&s_font, glyph_index, &advance, &lsb);
        (void)lsb;
        width += advance * em_scale;
        previous_glyph = glyph_index;
    }
    return width;
}

void ui_draw_text_centered(float x_center, float y, float scale, uint32_t color, const char* str) {
    float w = ui_get_text_width(scale, str);
    ui_draw_text(x_center - (w / 2.0f), y, scale, color, str);
}

void ui_draw_text_right(float x_right, float y, float scale, uint32_t color, const char* str) {
    float w = ui_get_text_width(scale, str);
    ui_draw_text(x_right - w, y, scale, color, str);
}

bool ui_wifi_connected(void);

static bool g_battery_linear = true;

void ui_set_battery_linear(bool linear) {
    g_battery_linear = linear;
}

static int battery_percent(void) {
    static int cached = -1;
    static time_t last_poll = 0;
    time_t now = (time_t)(osGetTime() / 1000);
    if (cached >= 0 && now - last_poll < 30) {
        return cached;
    }
    u8 percent = 0;
    if (R_SUCCEEDED(mcuHwcInit())) {
        if (R_SUCCEEDED(MCUHWC_GetBatteryLevel(&percent)) && percent <= 100) {
            cached = percent;
            last_poll = now;
        }
        mcuHwcExit();
    }
    if (cached < 0 && R_SUCCEEDED(ptmuInit())) {
        u8 level = 0;
        if (R_SUCCEEDED(PTMU_GetBatteryLevel(&level))) {
            cached = (int)level * 20;
            last_poll = now;
        }
        ptmuExit();
    }
    return cached;
}

static int system_battery_level(void) {
    static int cached = 0;
    static time_t last_poll = 0;
    time_t now = (time_t)(osGetTime() / 1000);
    if (last_poll != 0 && now - last_poll < 30) {
        return cached;
    }
    if (R_SUCCEEDED(ptmuInit())) {
        u8 level = 0;
        if (R_SUCCEEDED(PTMU_GetBatteryLevel(&level))) {
            cached = (int)level;
            last_poll = now;
        }
        ptmuExit();
    }
    return cached;
}

void ui_draw_wifi_icon(float x, float y, float size, bool connected) {
    uint32_t color = connected ? g_theme.review : g_theme.text_faint;
    float unit = size / 10.0f;
    for (int i = 0; i < 3; i++) {
        float height = unit * (3.0f + i * 2.5f);
        ui_draw_rounded_rect(x + i * unit * 3.0f, y + (size - height), unit * 2.2f,
                             height, unit, color);
    }
    if (!connected) {
        ui_draw_line(x, y + size, x + size, y, 1.5f, g_theme.text_faint);
    }
}

static void draw_arc(float cx, float cy, float radius, float start, float end, uint32_t color) {
    int steps = 4;
    float previous_x = cx + cosf(start) * radius;
    float previous_y = cy + sinf(start) * radius;
    for (int i = 1; i <= steps; i++) {
        float angle = start + (end - start) * (float)i / (float)steps;
        float x = cx + cosf(angle) * radius;
        float y = cy + sinf(angle) * radius;
        ui_draw_line(previous_x, previous_y, x, y, 1.6f, color);
        previous_x = x;
        previous_y = y;
    }
}

void ui_draw_sync_icon(float x, float y, float size, int state) {
    float cx = x + size * 0.5f;
    float cy = y + size * 0.5f;
    float radius = size * 0.40f;
    switch (state) {
        case 1:
            ui_draw_circle(cx, cy, radius * 0.62f, g_theme.hard_fg);
            break;
        case 2: {
            static int phase = 0;
            phase = (phase + 1) % 8;
            for (int i = 0; i < 8; i++) {
                if (i == phase || i == (phase + 7) % 8) continue;
                float start = (float)i * (2.0f * (float)M_PI / 8.0f);
                draw_arc(cx, cy, radius, start, start + (2.0f * (float)M_PI / 8.0f),
                         g_theme.accent);
            }
            break;
        }
        default:
            ui_draw_line(cx - radius * 0.75f, cy + radius * 0.05f, cx - radius * 0.15f,
                         cy + radius * 0.65f, 1.8f, g_theme.review);
            ui_draw_line(cx - radius * 0.15f, cy + radius * 0.65f, cx + radius * 0.8f,
                         cy - radius * 0.6f, 1.8f, g_theme.review);
            break;
    }
}

static void draw_status(float x_right, float y, float scale, int sync_state) {
    static const int64_t epoch_1900_to_1970 = 2208988800LL;
    time_t raw_time = (time_t)((int64_t)(osGetTime() / 1000) - epoch_1900_to_1970);
    struct tm ti;
    char time_str[16];
    if (gmtime_r(&raw_time, &ti)) {
        snprintf(time_str, sizeof(time_str), "%02d:%02d", ti.tm_hour, ti.tm_min);
    } else {
        snprintf(time_str, sizeof(time_str), "--:--");
    }

    float batt_w = 16.0f;
    float batt_h = 8.0f;
    float batt_x = x_right - batt_w;
    float midline = y + 7.0f;
    float batt_y = midline - batt_h * 0.5f;
    ui_draw_rounded_rect(batt_x, batt_y, batt_w, batt_h, 2.0f, g_theme.text_muted);
    ui_draw_rounded_rect(batt_x + 1, batt_y + 1, batt_w - 2, batt_h - 2, 1.5f, g_theme.bg_bottom);
    ui_draw_rect(batt_x + batt_w, batt_y + 2, 2, 4, g_theme.text_muted);

    int bars;
    if (g_battery_linear) {
        int percent = battery_percent();
        if (percent < 0) percent = system_battery_level() * 20;
        if (percent >= 75) {
            bars = 4;
        } else if (percent >= 50) {
            bars = 3;
        } else if (percent >= 25) {
            bars = 2;
        } else if (percent > 0) {
            bars = 1;
        } else {
            bars = 0;
        }
    } else {
        bars = system_battery_level();
        if (bars > 4) bars = 4;
    }
    uint32_t fill_clr = (bars <= 1) ? g_theme.learn : g_theme.review;
    for (int b = 0; b < bars; b++) {
        ui_draw_rect(batt_x + 2 + b * 3.0f, batt_y + 2, 2.2f, batt_h - 4, fill_clr);
    }

    float icon = 10.0f;
    float gap = 6.0f;
    float wifi_x = batt_x - gap - icon;
    float sync_x = wifi_x - gap - icon;
    float time_right = sync_x - gap;
    float text_y = midline - 0.48f * (float)card_font_px(scale);
    ui_draw_text_right(time_right, text_y, scale, g_theme.text_muted, time_str);
    ui_draw_sync_icon(sync_x, midline - icon * 0.5f, icon, sync_state);
    ui_draw_wifi_icon(wifi_x, midline - icon * 0.5f, icon, ui_wifi_connected());
}

void ui_draw_status_compact(float x_right, float y, float scale, int sync_state) {
    draw_status(x_right, y, scale, sync_state);
}

void ui_draw_scroll_indicator(float x, float y, float w, float h, float scroll, float max_scroll) {
    if (max_scroll <= 0.0f || h <= 0.0f) return;
    float ratio = scroll / max_scroll;
    if (ratio < 0.0f) ratio = 0.0f;
    if (ratio > 1.0f) ratio = 1.0f;
    float thumb_h = h * (h / (h + max_scroll));
    if (thumb_h < 22.0f) thumb_h = 22.0f;
    if (thumb_h > h) thumb_h = h;
    float thumb_y = y + (h - thumb_h) * ratio;
    float radius = w * 0.5f;
    ui_draw_rounded_rect(x, y, w, h, radius, g_theme.scroll_track);
    ui_draw_rounded_rect(x, thumb_y, w, thumb_h, radius, g_theme.scroll_thumb);
}

void ui_draw_beveled_panel(float x, float y, float w, float h, float radius, uint32_t bg_color, uint32_t border_color) {
    ui_draw_rect(x + 1.0f, y + 1.0f, w, h, C2D_RGBA(0, 0, 0, 25));
    ui_draw_rect(x, y, w, h, bg_color);
    ui_draw_line(x, y, x + w, y, 1.0f, border_color);
    ui_draw_line(x, y + h, x + w, y + h, 1.0f, border_color);
    ui_draw_line(x, y, x, y + h, 1.0f, border_color);
    ui_draw_line(x + w, y, x + w, y + h, 1.0f, border_color);
}

void ui_draw_plaque(float x, float y, float w, float h, const char* title, const char* subtitle) {
    ui_draw_rect(x + 2, y + 3, w, h, C2D_RGBA(0, 0, 0, 40));

    ui_draw_rect_gradient(x, y, w, h, C2D_RGBA(232, 235, 240, 255), C2D_RGBA(178, 184, 192, 255));

    ui_draw_line(x + 1, y + 1, x + w - 1, y + 1, 1.0f, C2D_RGBA(255, 255, 255, 220)); // top highlight
    ui_draw_line(x + 1, y + 1, x + 1, y + h - 1, 1.0f, C2D_RGBA(255, 255, 255, 180)); // left highlight
    ui_draw_line(x + 1, y + h - 1, x + w - 1, y + h - 1, 1.0f, C2D_RGBA(120, 126, 134, 255)); // bot shadow
    ui_draw_line(x + w - 1, y + 1, x + w - 1, y + h - 1, 1.0f, C2D_RGBA(120, 126, 134, 255)); // right shadow

    ui_draw_line(x, y, x + w, y, 1.0f, C2D_RGBA(90, 96, 104, 255));
    ui_draw_line(x, y + h, x + w, y + h, 1.0f, C2D_RGBA(90, 96, 104, 255));
    ui_draw_line(x, y, x, y + h, 1.0f, C2D_RGBA(90, 96, 104, 255));
    ui_draw_line(x + w, y, x + w, y + h, 1.0f, C2D_RGBA(90, 96, 104, 255));

    float rivet_offset = 6.0f;
    float rx[4] = { x + rivet_offset, x + w - rivet_offset, x + rivet_offset, x + w - rivet_offset };
    float ry[4] = { y + rivet_offset, y + rivet_offset, y + h - rivet_offset, y + h - rivet_offset };
    for (int i = 0; i < 4; i++) {
        ui_draw_circle(rx[i], ry[i], 2.5f, C2D_RGBA(110, 116, 124, 255));
        ui_draw_circle(rx[i] - 0.5f, ry[i] - 0.5f, 1.2f, C2D_RGBA(240, 240, 240, 255));
    }

    if (title && title[0]) {
        ui_draw_text_centered(x + (w / 2.0f), y + 12.0f, 0.65f, C2D_RGBA(20, 20, 20, 255), title);
    }
    if (subtitle && subtitle[0]) {
        ui_draw_text_centered(x + (w / 2.0f), y + 36.0f, 0.48f, C2D_RGBA(70, 75, 85, 255), subtitle);
    }
}

// Small outline icons share a 24-unit grid and rounded two-unit strokes.
// The stacked-card silhouette follows Anki's existing desktop deck icon.
// Render directly with Citro2D so no texture allocation or runtime SVG is needed.
static void icon_line(float x, float y, float scale, float x0, float y0,
                      float x1, float y1, uint32_t color) {
    float width = 2.0f * scale;
    ui_draw_line(x + x0 * scale, y + y0 * scale,
                 x + x1 * scale, y + y1 * scale, width, color);
    ui_draw_circle(x + x0 * scale, y + y0 * scale, width * 0.5f, color);
    ui_draw_circle(x + x1 * scale, y + y1 * scale, width * 0.5f, color);
}

static void icon_arc(float x, float y, float scale, float cx, float cy,
                     float radius, float start, float sweep, uint32_t color) {
    const int steps = 16;
    const float radians = 0.01745329252f;
    for (int i = 0; i < steps; i++) {
        float a = (start + sweep * i / steps) * radians;
        float b = (start + sweep * (i + 1) / steps) * radians;
        icon_line(x, y, scale, cx + cosf(a) * radius, cy + sinf(a) * radius,
                  cx + cosf(b) * radius, cy + sinf(b) * radius, color);
    }
}

static void icon_card(float x, float y, float scale, uint32_t color) {
    icon_line(x, y, scale, 5, 8, 14, 8, color);
    icon_line(x, y, scale, 16, 10, 16, 20, color);
    icon_line(x, y, scale, 14, 22, 5, 22, color);
    icon_line(x, y, scale, 3, 20, 3, 10, color);
    icon_arc(x, y, scale, 5, 10, 2, 180, 90, color);
    icon_arc(x, y, scale, 14, 10, 2, 270, 90, color);
    icon_arc(x, y, scale, 14, 20, 2, 0, 90, color);
    icon_arc(x, y, scale, 5, 20, 2, 90, 90, color);
    icon_line(x, y, scale, 7, 5, 19, 5, color);
    icon_line(x, y, scale, 19, 5, 19, 18, color);
    icon_line(x, y, scale, 10, 2, 22, 2, color);
    icon_line(x, y, scale, 22, 2, 22, 15, color);
}

void ui_draw_icon_help(float x, float y, float size) {
    float scale = size / 24.0f;
    uint32_t color = C2D_RGBA(37, 99, 235, 255);
    icon_arc(x, y, scale, 12, 12, 9, 0, 360, color);
    icon_arc(x, y, scale, 12, 10, 3, 200, 250, color);
    icon_line(x, y, scale, 12, 13, 12, 14, color);
    ui_draw_circle(x + 12 * scale, y + 18 * scale, scale, color);
}

void ui_draw_icon_stats(float x, float y, float size) {
    float scale = size / 24.0f;
    uint32_t color = C2D_RGBA(37, 99, 235, 255);
    icon_line(x, y, scale, 4, 20, 20, 20, color);
    icon_line(x, y, scale, 6, 15, 6, 17, color);
    icon_line(x, y, scale, 12, 10, 12, 17, color);
    icon_line(x, y, scale, 18, 4, 18, 17, color);
}

void ui_draw_icon_gear(float x, float y, float size) {
    float scale = size / 24.0f;
    uint32_t color = C2D_RGBA(37, 99, 235, 255);
    icon_arc(x, y, scale, 12, 12, 6, 0, 360, color);
    icon_arc(x, y, scale, 12, 12, 2, 0, 360, color);
    for (int i = 0; i < 8; i++) {
        float angle = i * 0.7853981634f;
        icon_line(x, y, scale, 12 + cosf(angle) * 6, 12 + sinf(angle) * 6,
                  12 + cosf(angle) * 9, 12 + sinf(angle) * 9, color);
    }
}

void ui_draw_nav_tab(float x, float y, float w, float h, const char* label, int icon_type, bool active) {
    ui_draw_rect(x, y, w, h, g_theme.surface_alt);
    if (active) {
        ui_draw_rounded_rect(x + 8.0f, y + 3.0f, w - 16.0f, h - 6.0f, 14.0f, g_theme.tab_active_bg);
    }

    float icon_cx = x + (w / 2.0f);
    float icon_cy = y + 13.0f;
    float ix = icon_cx - 10.0f;
    float iy = icon_cy - 10.0f;
    float scale = 20.0f / 24.0f;
    uint32_t color = active ? g_theme.tab_active_fg : g_theme.tab_inactive_fg;
    if (icon_type == 0) {
        icon_card(ix, iy, scale, color);
    } else if (icon_type == 1) {
        icon_line(ix, iy, scale, 12, 4, 12, 20, color);
        icon_line(ix, iy, scale, 4, 12, 20, 12, color);
    } else if (icon_type == 2) {
        icon_line(ix, iy, scale, 4, 16, 16, 4, color);
        icon_line(ix, iy, scale, 16, 4, 20, 8, color);
        icon_line(ix, iy, scale, 20, 8, 8, 20, color);
        icon_line(ix, iy, scale, 8, 20, 3, 21, color);
        icon_line(ix, iy, scale, 3, 21, 4, 16, color);
        icon_line(ix, iy, scale, 13, 7, 17, 11, color);
    } else if (icon_type == 3) {
        icon_arc(ix, iy, scale, 10, 10, 6, 0, 360, color);
        icon_line(ix, iy, scale, 14.5f, 14.5f, 21, 21, color);
    } else if (icon_type == 4) {
        icon_line(ix, iy, scale, 4, 7, 20, 7, color);
        ui_draw_circle(ix + 15 * scale, iy + 7 * scale, 2.4f * scale, color);
        icon_line(ix, iy, scale, 4, 12, 20, 12, color);
        ui_draw_circle(ix + 9 * scale, iy + 12 * scale, 2.4f * scale, color);
        icon_line(ix, iy, scale, 4, 17, 20, 17, color);
        ui_draw_circle(ix + 15 * scale, iy + 17 * scale, 2.4f * scale, color);
    }

    ui_draw_text_centered(icon_cx, y + 26.0f, 0.42f, color, label);
}

void ui_draw_deck_row(float y, float w, const char* name, int level, bool has_children, bool expanded,
                     uint32_t due_count, uint32_t new_count, uint32_t learn_count, bool selected) {
    float h = 24.0f;
    ui_draw_rounded_rect(5.0f, y + 1.0f, w - 10.0f, h - 2.0f, 8.0f,
                         selected ? g_theme.row_selected : g_theme.surface);

    float indent = 12.0f + (float)level * 14.0f;

    if (has_children) {
        float cx = indent + 4.0f;
        float cy = y + h * 0.5f;
        uint32_t arrow = selected ? g_theme.tab_active_fg : g_theme.text_muted;
        if (expanded) {
            ui_draw_triangle(cx - 5.0f, cy - 2.5f, cx + 5.0f, cy - 2.5f, cx, cy + 3.5f, arrow);
        } else {
            ui_draw_triangle(cx - 2.5f, cy - 5.0f, cx - 2.5f, cy + 5.0f, cx + 3.5f, cy, arrow);
        }
        indent += 15.0f;
    }

    float max_name_w = (w - 132.0f) - indent;
    char disp_name[1024];
    size_t bytes = strlen(name);
    if (bytes > sizeof(disp_name) - 4) bytes = sizeof(disp_name) - 4;
    while (bytes && ((unsigned char)name[bytes] & 0xc0) == 0x80) bytes--;
    memcpy(disp_name, name, bytes);
    disp_name[bytes] = '\0';
    if (ui_get_text_width(0.50f, disp_name) > max_name_w) {
        size_t ends[1024], n = 0;
        for (size_t i = 0; i < bytes; i++) {
            if (((unsigned char)name[i] & 0xc0) != 0x80) ends[n++] = i;
        }
        size_t low = 0, high = n;
        while (low < high) {
            size_t mid = (low + high + 1) / 2;
            size_t cut = mid < n ? ends[mid] : bytes;
            memcpy(disp_name, name, cut);
            strcpy(disp_name + cut, "...");
            if (ui_get_text_width(0.50f, disp_name) <= max_name_w) low = mid;
            else high = mid - 1;
        }
        size_t cut = low < n ? ends[low] : bytes;
        memcpy(disp_name, name, cut); strcpy(disp_name + cut, "...");
    }

    ui_draw_text(indent, y + 4.0f, 0.50f, g_theme.text, disp_name);

    char count_buf[16];
    uint32_t off = g_theme.text_faint;

    snprintf(count_buf, sizeof(count_buf), "%u", (unsigned int)new_count);
    ui_draw_text_right(w - 118.0f, y + 5.0f, 0.40f, new_count > 0 ? g_theme.new_color : off, count_buf);

    snprintf(count_buf, sizeof(count_buf), "%u", (unsigned int)learn_count);
    ui_draw_text_right(w - 72.0f, y + 5.0f, 0.40f, learn_count > 0 ? g_theme.learn : off, count_buf);

    snprintf(count_buf, sizeof(count_buf), "%u", (unsigned int)due_count);
    ui_draw_text_right(w - 22.0f, y + 5.0f, 0.40f, due_count > 0 ? g_theme.review : off, count_buf);
}

void ui_draw_scrollbar(float x, float y, float w, float h, int total_items, int visible_items, int scroll_pos) {
    if (total_items <= visible_items || total_items <= 0 || h <= 0.0f) return;
    float track_w = 4.0f;
    float tx = x + (w - track_w) * 0.5f;
    float radius = track_w * 0.5f;
    ui_draw_rounded_rect(tx, y, track_w, h, radius, g_theme.scroll_track);

    float thumb_h = h * ((float)visible_items / (float)total_items);
    if (thumb_h < 24.0f) thumb_h = 24.0f;
    if (thumb_h > h) thumb_h = h;
    float max_scroll = (float)(total_items - visible_items);
    float scroll_ratio = (float)scroll_pos / max_scroll;
    if (scroll_ratio < 0.0f) scroll_ratio = 0.0f;
    if (scroll_ratio > 1.0f) scroll_ratio = 1.0f;
    float thumb_y = y + (h - thumb_h) * scroll_ratio;
    ui_draw_rounded_rect(tx, thumb_y, track_w, thumb_h, radius, g_theme.scroll_thumb);
}

void ui_draw_button(float x, float y, float w, float h, const char* text, const char* subtext, int style, bool pressed) {
    uint32_t bg, fg;
    bool rating = (style >= 2 && style <= 5);
    bool outlined = (style == 0 || style == 6 || style == 8);
    switch (style) {
        case 0:
            bg = mix_color(g_theme.surface_alt, g_theme.accent, 0.16f);
            fg = g_theme.accent;
            break;
        case 1:
            bg = g_theme.hard_bg;
            fg = g_theme.hard_fg;
            break;
        case 2:
            bg = g_theme.again_bg;
            fg = g_theme.again_fg;
            break;
        case 3:
            bg = g_theme.hard_bg;
            fg = g_theme.hard_fg;
            break;
        case 4:
            bg = g_theme.good_bg;
            fg = g_theme.good_fg;
            break;
        case 5:
            bg = g_theme.easy_bg;
            fg = g_theme.easy_fg;
            break;
        case 8:
            bg = g_theme.surface_alt;
            fg = g_theme.text_faint;
            break;
        default:
            bg = g_theme.surface_alt;
            fg = g_theme.text;
            break;
    }
    if (pressed) bg = darken(bg, 0.10f);

    float radius = h * 0.5f;
    if (radius > 16.0f) radius = 16.0f;
    ui_draw_rounded_rect(x, y + 1.5f, w, h, radius, g_theme.shadow);
    if (outlined) {
        uint32_t border = style == 0
            ? mix_color(g_theme.surface_alt, g_theme.accent, 0.45f)
            : g_theme.border;
        ui_draw_rounded_rect(x, y, w, h, radius, border);
        ui_draw_rounded_rect(x + 1.0f, y + 1.0f, w - 2.0f, h - 2.0f,
                             radius - 1.0f > 1.0f ? radius - 1.0f : 1.0f, bg);
    } else {
        ui_draw_rounded_rect(x, y, w, h, radius, bg);
    }

    if (subtext && subtext[0]) {
        const char* top_line = rating ? subtext : text;
        const char* bottom_line = rating ? text : subtext;
        uint32_t top_clr = rating ? mix_color(fg, bg, 0.35f) : fg;
        ui_draw_text_centered(x + (w / 2.0f), y + 5.0f, 0.40f, top_clr, top_line);
        ui_draw_text_centered(x + (w / 2.0f), y + 19.0f, 0.46f, fg, bottom_line);
    } else {
        float scale = 0.50f;
        float text_width = ui_get_text_width(scale, text);
        float available = w - 12.0f;
        if (available > 0.0f && text_width > available) {
            scale *= available / text_width;
            if (scale < 0.30f) scale = 0.30f;
        }
        ui_draw_text_centered(x + (w / 2.0f), y + (h / 2.0f) - 6.0f, scale, fg, text);
    }
}

int ui_keyboard(const char* hint, const char* initial, char* output, size_t capacity, bool secret, const char *cancel, const char *accept) {
    SwkbdState keyboard;
    swkbdInit(&keyboard, SWKBD_TYPE_NORMAL, 2, (int)(capacity / 4 - 1));
    if (secret) swkbdSetPasswordMode(&keyboard, SWKBD_PASSWORD_HIDE);
    swkbdSetHintText(&keyboard, hint);
    swkbdSetInitialText(&keyboard, initial);
    swkbdSetButton(&keyboard, SWKBD_BUTTON_LEFT, cancel, false);
    swkbdSetButton(&keyboard, SWKBD_BUTTON_RIGHT, accept, true);
    swkbdSetValidation(&keyboard, SWKBD_ANYTHING, 0, 0);
    SwkbdButton result = swkbdInputText(&keyboard, output, capacity);
    if (result == SWKBD_BUTTON_RIGHT) return 1;
    if (result == SWKBD_BUTTON_LEFT) return 0;
    return -1;
}

bool ui_wifi_connected(void) {
    static uint64_t next_check;
    static bool connected;
    uint64_t now = svcGetSystemTick();
    if (now < next_check) return connected;
    next_check = now + SYSCLOCK_ARM11;
    u32 status = 0;
    if (R_FAILED(acInit())) return osGetWifiStrength() != 0;
    Result result = ACU_GetStatus(&status);
    acExit();
    connected = R_SUCCEEDED(result) && status == 3;
    return connected;
}
