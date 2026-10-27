// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

#include "ui_renderer.h"
#include <3ds.h>
#include <citro2d.h>
#include <citro3d.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define STB_TRUETYPE_IMPLEMENTATION
#include "stb_truetype.h"

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

static size_t swizzled_offset(int x, int y) {
    init_swizzle_map();
    int tile_x = x >> 3;
    int tile_y = y >> 3;
    int tiles_per_row = CARD_ATLAS_W >> 3;
    int tile_index = tile_y * tiles_per_row + tile_x;
    int pixel = (y & 7) * 8 + (x & 7);
    return ((size_t)tile_index * 64 + s_swizzle_map[pixel]) * 4;
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
            C3D_TexSetFilter(&s_card_atlas_tex, GPU_LINEAR, GPU_LINEAR);
            C3D_TexSetWrap(&s_card_atlas_tex, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
            reset_card_atlas();
            commit_card_atlas();
        } else {
            s_font_loaded = false;
        }
    }

    s_initialized = true;
    return true;
}

void ui_fini(void) {
    if (!s_initialized) return;

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
    C2D_DrawLine(x0, y0, color, x1, y1, color, thickness, 0.5f);
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
    C2D_DrawText(&text, C2D_WithColor, x, y, 0.6f, scale, scale, color);
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
    C2D_DrawText(&text, C2D_WithColor, x, y, 0.6f, scale, scale, color);
    if (bold) {
        C2D_DrawText(&text, C2D_WithColor, x + 0.7f, y, 0.6f, scale, scale, color);
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
    C2D_DrawImageAt(image, x + glyph->offset_x, y + glyph->offset_y, 0.6f, &tint, 1.0f, 1.0f);
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

static void draw_clock_battery(float x_right, float y, float scale) {
    static const int64_t epoch_1900_to_1970 = 2208988800LL;
    time_t raw_time = (time_t)((int64_t)(osGetTime() / 1000) - epoch_1900_to_1970);
    struct tm ti;
    char time_str[16];
    if (gmtime_r(&raw_time, &ti)) {
        snprintf(time_str, sizeof(time_str), "%02d:%02d", ti.tm_hour, ti.tm_min);
    } else {
        snprintf(time_str, sizeof(time_str), "--:--");
    }

    static u8 cached_batt = 0;
    static time_t last_poll = 0;
    u8 batt = cached_batt;
    if (raw_time != last_poll && R_SUCCEEDED(ptmuInit())) {
        if (R_SUCCEEDED(PTMU_GetBatteryLevel(&batt))) cached_batt = batt;
        ptmuExit();
        last_poll = raw_time;
    }

    float batt_w = 16.0f;
    float batt_h = 8.0f;
    float batt_x = x_right - batt_w;
    float batt_y = y + 1.0f;
    ui_draw_rounded_rect(batt_x, batt_y, batt_w, batt_h, 2.0f, g_theme.text_muted);
    ui_draw_rounded_rect(batt_x + 1, batt_y + 1, batt_w - 2, batt_h - 2, 1.5f, g_theme.bg_bottom);
    ui_draw_rect(batt_x + batt_w, batt_y + 2, 2, 4, g_theme.text_muted);

    int bars = (int)batt;
    if (bars > 4) bars = 4;
    uint32_t fill_clr = (bars <= 1) ? g_theme.learn : g_theme.review;
    for (int b = 0; b < bars; b++) {
        ui_draw_rect(batt_x + 2 + b * 3.0f, batt_y + 2, 2.2f, batt_h - 4, fill_clr);
    }

    ui_draw_text_right(batt_x - 5.0f, y, scale, g_theme.text_muted, time_str);
}

void ui_draw_status_compact(float x_right, float y, float scale) {
    draw_clock_battery(x_right, y, scale);
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
    switch (style) {
        case 0:
            bg = g_theme.accent;
            fg = g_theme.on_accent;
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
        default:
            bg = g_theme.surface_alt;
            fg = g_theme.text;
            break;
    }
    if (pressed) bg = darken(bg, style == 0 ? 0.18f : 0.10f);

    float radius = h * 0.5f;
    if (radius > 16.0f) radius = 16.0f;
    ui_draw_rounded_rect(x, y + 1.5f, w, h, radius, g_theme.shadow);
    ui_draw_rounded_rect(x, y, w, h, radius, bg);

    if (subtext && subtext[0]) {
        const char* top_line = rating ? subtext : text;
        const char* bottom_line = rating ? text : subtext;
        uint32_t top_clr = rating ? mix_color(fg, bg, 0.35f) : fg;
        ui_draw_text_centered(x + (w / 2.0f), y + 5.0f, 0.40f, top_clr, top_line);
        ui_draw_text_centered(x + (w / 2.0f), y + 19.0f, 0.46f, fg, bottom_line);
    } else {
        ui_draw_text_centered(x + (w / 2.0f), y + (h / 2.0f) - 6.0f, 0.50f, fg, text);
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
