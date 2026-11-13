use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{
    div, point, prelude::*, px, rgb, size, App, Application, Bounds, ClipboardItem, Context,
    MouseButton, Pixels, SharedString, Size, Timer, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions,
};

const APP_ID: &str = "gpui-clipboard";

/// Cinnamon bottom panel height on this setup (`panels-height` = 48).
const PANEL_HEIGHT: f32 = 48.0;
const EDGE_MARGIN: f32 = 12.0;
const WINDOW_WIDTH: f32 = 420.0;
const LINE_HEIGHT: f32 = 22.0;
/// Vertical padding inside a row (`py`); keep in sync with `.py(px(PAD_Y))`.
const PAD_Y: f32 = 4.0;
const BORDER_WIDTH: f32 = 1.0;
/// Top/bottom padding + borders around the text block.
const VERTICAL_CHROME: f32 = PAD_Y * 2.0 + BORDER_WIDTH * 2.0;
const ROW_GAP: f32 = 4.0;
const MAX_LINES: usize = 10;
const MAX_HISTORY: usize = 5;

const OPACITY_ACTIVE: f32 = 1.0;
const OPACITY_INACTIVE: f32 = 0.6;
const HIDE_DELAY: Duration = Duration::from_millis(300);
const HIDE_FADE: Duration = Duration::from_millis(500);

const BORDER_CURRENT: u32 = 0x88c0d0;
const BORDER_HISTORY: u32 = 0x5e81ac;
const BG: u32 = 0x2e3440;

struct ClipboardLine {
    /// Full clipboard text; blank/whitespace-only is treated as empty.
    current: String,
    /// Newest previous state at the front.
    history: VecDeque<String>,
    expanded: bool,
    hovered: bool,
    /// History stays visible during leave delay/fade.
    show_history: bool,
    opacity: f32,
    /// Set when the cursor leaves while history is visible.
    unhover_since: Option<Instant>,
}

impl ClipboardLine {
    fn new(window: &Window, cx: &mut Context<Self>) -> Self {
        cx.spawn_in(window, async move |this, cx| loop {
            Timer::after(Duration::from_millis(250)).await;

            let text = cx
                .update(|_, cx| read_clipboard_text(cx))
                .unwrap_or_default();

            let updated = this.update_in(cx, |view, window, cx| {
                if view.current != text {
                    let previous = std::mem::replace(&mut view.current, text);
                    view.push_history(previous);
                    if view.display_total_lines() <= 1 {
                        view.expanded = false;
                    }
                    view.apply_window_bounds(window, cx);
                    cx.notify();
                }
            });

            if updated.is_err() {
                break;
            }
        })
        .detach();

        Self {
            current: read_clipboard_text(cx),
            history: VecDeque::new(),
            expanded: false,
            hovered: false,
            show_history: false,
            opacity: OPACITY_INACTIVE,
            unhover_since: None,
        }
    }

    fn push_history(&mut self, previous: String) {
        if is_blank(&previous) {
            return;
        }
        // Drop duplicates of the new current that may linger after a restore.
        self.history.retain(|entry| entry != &self.current);
        if self.history.front().is_some_and(|front| front == &previous) {
            return;
        }
        self.history.push_front(previous);
        while self.history.len() > MAX_HISTORY {
            self.history.pop_back();
        }
    }

    fn display_lines(text: &str) -> Vec<SharedString> {
        if is_blank(text) {
            return Vec::new();
        }
        text.lines()
            .take(MAX_LINES)
            .map(|line| SharedString::from(line.to_string()))
            .collect()
    }

    fn display_total_lines(&self) -> usize {
        if is_blank(&self.current) {
            0
        } else {
            self.current.lines().count()
        }
    }

    fn current_visible_line_count(&self) -> usize {
        let total = self.display_total_lines();
        if total == 0 {
            1
        } else if self.expanded {
            total.min(MAX_LINES)
        } else {
            1
        }
    }

    fn collapsed_row_height() -> f32 {
        VERTICAL_CHROME + LINE_HEIGHT
    }

    fn window_size(&self) -> Size<Pixels> {
        let history_count = if self.show_history {
            self.history.len() as f32
        } else {
            0.0
        };
        let history_height = if history_count == 0.0 {
            0.0
        } else {
            history_count * Self::collapsed_row_height() + history_count * ROW_GAP
        };
        let current_height =
            VERTICAL_CHROME + self.current_visible_line_count() as f32 * LINE_HEIGHT;

        size(px(WINDOW_WIDTH), px(history_height + current_height))
    }

    fn apply_window_bounds(&self, window: &mut Window, cx: &App) {
        let bounds = anchored_bounds(self.window_size(), cx);
        place_app_window(bounds, window.scale_factor());
    }

    fn sync_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hovered = window.is_window_hovered();

        if hovered {
            let need_resize = !self.show_history;
            self.hovered = true;
            self.unhover_since = None;
            self.show_history = true;
            self.opacity = OPACITY_ACTIVE;
            if need_resize {
                self.apply_window_bounds(window, cx);
            }
            return;
        }

        self.hovered = false;

        if !self.show_history {
            self.opacity = OPACITY_INACTIVE;
            self.unhover_since = None;
            return;
        }

        let since = *self.unhover_since.get_or_insert_with(Instant::now);
        let elapsed = since.elapsed();

        if elapsed < HIDE_DELAY {
            self.opacity = OPACITY_ACTIVE;
            window.request_animation_frame();
            return;
        }

        if elapsed < HIDE_DELAY + HIDE_FADE {
            let t =
                ((elapsed - HIDE_DELAY).as_secs_f32() / HIDE_FADE.as_secs_f32()).clamp(0.0, 1.0);
            self.opacity = OPACITY_ACTIVE + (OPACITY_INACTIVE - OPACITY_ACTIVE) * t;
            window.request_animation_frame();
            return;
        }

        self.show_history = false;
        self.expanded = false;
        self.opacity = OPACITY_INACTIVE;
        self.unhover_since = None;
        self.apply_window_bounds(window, cx);
    }

    fn toggle_expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.display_total_lines() <= 1 {
            return;
        }
        self.expanded = !self.expanded;
        self.apply_window_bounds(window, cx);
        cx.notify();
    }

    fn restore_history(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.history.remove(index) else {
            return;
        };
        let previous = std::mem::replace(&mut self.current, text.clone());
        self.push_history(previous);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.apply_window_bounds(window, cx);
        cx.notify();
    }

    fn clear_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if is_blank(&self.current) {
            return;
        }
        self.current.clear();
        self.expanded = false;
        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        self.apply_window_bounds(window, cx);
        cx.notify();
    }

    fn remove_history(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.remove(index).is_none() {
            return;
        }
        self.apply_window_bounds(window, cx);
        cx.notify();
    }
}

impl Render for ClipboardLine {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_hover(window, cx);

        let history_rows = if self.show_history {
            self.history
                .iter()
                .enumerate()
                .rev()
                .map(|(index, text)| {
                    let preview = preview_label(text);
                    div()
                        .id(("history", index))
                        .flex()
                        .flex_row()
                        .items_start()
                        .px_3()
                        .py(px(PAD_Y))
                        .h(px(Self::collapsed_row_height()))
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER_HISTORY))
                        .bg(rgb(BG))
                        .font_family("FiraCode Nerd Font Mono")
                        .text_size(px(16.))
                        .text_color(rgb(0xe8e8e8))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.restore_history(index, window, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, window, cx| {
                                this.remove_history(index, window, cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .h(px(LINE_HEIGHT))
                                .flex()
                                .items_center()
                                .truncate()
                                .child(preview),
                        )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        div()
            .id("app")
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .size_full()
            .opacity(self.opacity)
            .children(history_rows)
            .child(self.render_current(cx))
    }
}

impl ClipboardLine {
    fn render_current(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = Self::display_lines(&self.current);
        let total_lines = self.display_total_lines();
        let color = if lines.is_empty() {
            rgb(0x777777)
        } else {
            rgb(0xe8e8e8)
        };

        let show_badge = total_lines > 1;
        let show_count = if self.expanded {
            total_lines > lines.len()
        } else {
            total_lines > 1
        };

        let body = if lines.is_empty() {
            div()
                .h(px(LINE_HEIGHT))
                .flex()
                .items_center()
                .truncate()
                .child(SharedString::from("empty"))
        } else if self.expanded {
            div()
                .flex()
                .flex_col()
                .children(lines.into_iter().map(|line| {
                    div()
                        .h(px(LINE_HEIGHT))
                        .flex()
                        .items_center()
                        .truncate()
                        .child(line)
                }))
        } else {
            div()
                .h(px(LINE_HEIGHT))
                .flex()
                .items_center()
                .truncate()
                .child(lines[0].clone())
        };

        let row_height =
            VERTICAL_CHROME + self.current_visible_line_count() as f32 * LINE_HEIGHT;

        div()
            .id("clipboard")
            .flex()
            .flex_row()
            .items_start()
            .px_3()
            .py(px(PAD_Y))
            .w_full()
            .h(px(row_height))
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER_CURRENT))
            .bg(rgb(BG))
            .font_family("FiraCode Nerd Font Mono")
            .text_size(px(16.))
            .text_color(color)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.toggle_expanded(window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| this.clear_current(window, cx)),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().child(body))
            .when(show_badge, |el| {
                let chevron = if self.expanded { "▴" } else { "▾" };
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(LINE_HEIGHT))
                        .gap_1()
                        .pl_2()
                        .text_color(rgb(BORDER_CURRENT))
                        .when(show_count, |el| el.child(format!("{total_lines}")))
                        .child(chevron),
                )
            })
    }
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

fn preview_label(text: &str) -> SharedString {
    if is_blank(text) {
        SharedString::from("empty")
    } else {
        SharedString::from(text.lines().next().unwrap_or("").to_string())
    }
}

fn read_clipboard_text(cx: &App) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// Bottom-right of the primary display, with a fixed gap above the Cinnamon panel.
fn anchored_bounds(window_size: Size<Pixels>, cx: &App) -> Bounds<Pixels> {
    let screen = cx
        .primary_display()
        .map(|d| d.bounds())
        .unwrap_or_else(|| Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1920.), px(1080.)),
        });

    let clearance = px(PANEL_HEIGHT + EDGE_MARGIN);
    let margin = px(EDGE_MARGIN);

    Bounds {
        origin: screen.bottom_right()
            - point(window_size.width + margin, window_size.height + clearance),
        size: window_size,
    }
}

fn place_app_window(bounds: Bounds<Pixels>, scale: f32) {
    let x = (f32::from(bounds.origin.x) * scale).round() as i32;
    let y = (f32::from(bounds.origin.y) * scale).round() as i32;
    let width = (f32::from(bounds.size.width) * scale).round().max(1.0) as u32;
    let height = (f32::from(bounds.size.height) * scale).round().max(1.0) as u32;

    // wmctrl speaks EWMH (_NET_MOVERESIZE_WINDOW). Direct ConfigureWindow is ignored by
    // Cinnamon when moving the window down, and gpui's X11 HasWindowHandle panics.
    let geometry = format!("0,{x},{y},{width},{height}");
    let class = format!("{APP_ID}.{APP_ID}");
    let _ = std::process::Command::new("wmctrl")
        .args(["-x", "-r", &class, "-e", &geometry])
        .status();
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = anchored_bounds(
            size(px(WINDOW_WIDTH), px(ClipboardLine::collapsed_row_height())),
            cx,
        );
        let display_id = cx.primary_display().map(|d| d.id());

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id,
                titlebar: None,
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: Some(WindowDecorations::Client),
                kind: WindowKind::PopUp,
                focus: false,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                app_id: Some(APP_ID.into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ClipboardLine::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
