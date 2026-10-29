// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

#ifndef ANKI_3DS_UI_RENDERER_H
#define ANKI_3DS_UI_RENDERER_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TOP_WIDTH 400
#define TOP_HEIGHT 240
#define BOTTOM_WIDTH 320
#define BOTTOM_HEIGHT 240

#define C2D_RGBA(r, g, b, a) ((uint32_t)(r) | ((uint32_t)(g) << 8) | ((uint32_t)(b) << 16) | ((uint32_t)(a) << 24))

typedef struct {
    uint32_t bg_top;
    uint32_t bg_bottom;
    uint32_t surface;
    uint32_t surface_alt;
    uint32_t surface_pressed;
    uint32_t border;
    uint32_t text;
    uint32_t text_muted;
    uint32_t text_faint;
    uint32_t accent;
    uint32_t on_accent;
    uint32_t new_color;
    uint32_t learn;
    uint32_t review;
    uint32_t cloze_bg;
    uint32_t pitch;
    uint32_t row_selected;
    uint32_t tab_active_bg;
    uint32_t tab_active_fg;
    uint32_t tab_inactive_bg;
    uint32_t tab_inactive_fg;
    uint32_t scroll_track;
    uint32_t scroll_thumb;
    uint32_t again_bg;
    uint32_t again_fg;
    uint32_t hard_bg;
    uint32_t hard_fg;
    uint32_t good_bg;
    uint32_t good_fg;
    uint32_t easy_bg;
    uint32_t easy_fg;
    uint32_t shadow;
} UiTheme;

// `font_data` is the bundled TrueType font and must remain valid for the
// lifetime of the UI. The renderer rasterizes CJK glyphs into a small atlas.
bool ui_init(const void* font_data, size_t font_size, const void* fallback_data,
             size_t fallback_size);
void ui_set_theme(const UiTheme* theme);
void ui_fini(void);
void ui_frame_begin(void);
void ui_frame_end(void);

void ui_target_top(uint32_t bg_color);
void ui_target_bottom(uint32_t bg_color);

void ui_draw_rect(float x, float y, float w, float h, uint32_t color);
void ui_draw_rect_gradient(float x, float y, float w, float h, uint32_t c_top, uint32_t c_bot);
void ui_draw_line(float x0, float y0, float x1, float y1, float thickness, uint32_t color);
void ui_draw_circle(float x, float y, float radius, uint32_t color);
void ui_draw_rounded_rect(float x, float y, float w, float h, float radius, uint32_t color);
void ui_draw_triangle(float x0, float y0, float x1, float y1, float x2, float y2, uint32_t color);

void ui_draw_text(float x, float y, float scale, uint32_t color, const char* str);
void ui_draw_text_centered(float x_center, float y, float scale, uint32_t color, const char* str);
void ui_draw_text_right(float x_right, float y, float scale, uint32_t color, const char* str);
float ui_get_text_width(float scale, const char* str);

// Card text uses the bundled TrueType font. Callers should prepare all
// visible text first, then commit once, so a frame needs at most one atlas
// upload. Bold is approximated by a second pass.
void ui_prepare_card_text(float scale, const char* str);
bool ui_prepare_card_text_checked(float scale, const char* str);
void ui_commit_card_text(void);
void ui_reset_card_text(void);
void ui_draw_card_text(float x, float y, float scale, uint32_t color, const char* str, bool bold, bool italic);
float ui_get_card_text_width(float scale, const char* str);

// Compact clock + battery cluster without the old status-bar banner.
void ui_draw_status_compact(float x_right, float y, float scale, int sync_state);
void ui_draw_sync_icon(float x, float y, float size, int state);
void ui_draw_wifi_icon(float x, float y, float size, bool connected);
void ui_set_battery_linear(bool linear);
// Minimal scroll thumb for full-screen content.
void ui_draw_scroll_indicator(float x, float y, float w, float h, float scroll, float max_scroll);

void ui_draw_beveled_panel(float x, float y, float w, float h, float radius, uint32_t bg_color, uint32_t border_color);
void ui_draw_plaque(float x, float y, float w, float h, const char* title, const char* subtitle);
void ui_draw_nav_tab(float x, float y, float w, float h, const char* label, int icon_type, bool active);
void ui_draw_deck_row(float y, float w, const char* name, int level, bool has_children, bool expanded,
                     uint32_t due_count, uint32_t new_count, uint32_t learn_count, bool selected);
void ui_draw_scrollbar(float x, float y, float w, float h, int total_items, int visible_items, int scroll_pos);
void ui_draw_button(float x, float y, float w, float h, const char* text, const char* subtext, int style, bool pressed);

int ui_media_image(const char* path, int* width, int* height);
void ui_media_draw(int id, float x, float y, float w, float h);
void ui_media_reset(void);
void ui_media_refresh(void);
bool ui_media_prefetch(const char* path);
void ui_media_prefetch_clear(void);

bool ui_audio_play(const char* path);
bool ui_audio_decode(const char* path);
bool ui_audio_prefetch(const char* path);
void ui_audio_prefetch_clear(void);
bool ui_audio_is_playing(void);
void ui_audio_stop(void);
void ui_audio_init(void);
void ui_audio_fini(void);

void ui_draw_icon_help(float x, float y, float size);
void ui_draw_icon_stats(float x, float y, float size);
void ui_draw_icon_gear(float x, float y, float size);

#ifdef __cplusplus
}
#endif

#endif // ANKI_3DS_UI_RENDERER_H
