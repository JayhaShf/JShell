use alacritty_terminal::{
    term::cell::Flags,
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor},
};
use gpui::{
    App, Bounds, Element, ElementId, Entity, FocusHandle, Font, GlobalElementId, Hsla,
    InputHandler, IntoElement, LayoutId, Pixels, Point, Rgba, SharedString, StrikethroughStyle,
    TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, fill, point, px, relative, rgb,
};
use gpui_component::ActiveTheme as _;

use crate::Ashell;
use crate::terminal::custom_blocks::{is_custom_block_supported, paint_custom_block};
use crate::terminal::{RenderSnapshot, ViewportSelection};

#[derive(Clone, Copy)]
struct TerminalMetrics {
    cell_width: Pixels,
    line_height: Pixels,
}

fn terminal_cell_span(flags: Flags) -> usize {
    if flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

#[derive(Clone)]
struct LayoutRect {
    row: i32,
    col: i32,
    cells: usize,
    color: Hsla,
}

impl LayoutRect {
    fn paint(&self, origin: Point<Pixels>, metrics: TerminalMetrics, window: &mut Window) {
        let position = point(
            origin.x + metrics.cell_width * self.col as f32,
            origin.y + metrics.line_height * self.row as f32,
        );
        let size = gpui::size(metrics.cell_width * self.cells as f32, metrics.line_height);
        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

#[derive(Clone)]
struct LayoutUnderline {
    row: i32,
    col: i32,
    cells: usize,
    color: Hsla,
}

impl LayoutUnderline {
    fn paint(&self, origin: Point<Pixels>, metrics: TerminalMetrics, window: &mut Window) {
        let thickness = px(1.0);
        let position = point(
            origin.x + metrics.cell_width * self.col as f32,
            origin.y + metrics.line_height * (self.row as f32 + 1.0) - thickness,
        );
        let size = gpui::size(metrics.cell_width * self.cells as f32, thickness);
        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

#[derive(Clone)]
struct BatchedTextRun {
    row: i32,
    col: i32,
    cell_count: usize,
    text: String,
    style: TextRun,
    font_size: Pixels,
}

impl BatchedTextRun {
    fn new(row: i32, col: i32, ch: char, style: TextRun, font_size: Pixels) -> Self {
        Self {
            row,
            col,
            cell_count: 1,
            text: ch.to_string(),
            style,
            font_size,
        }
    }

    fn can_append(&self, other: &TextRun, row: i32, col: i32) -> bool {
        self.row == row
            && self.col + self.cell_count as i32 == col
            && self.style.font == other.font
            && self.style.color == other.color
            && self.style.background_color == other.background_color
            && self.style.underline == other.underline
            && self.style.strikethrough == other.strikethrough
    }

    fn append(&mut self, ch: char, zerowidth: Option<&[char]>) {
        self.text.push(ch);
        self.cell_count += 1;
        self.style.len += ch.len_utf8();
        if let Some(chars) = zerowidth {
            for c in chars {
                self.text.push(*c);
                self.style.len += c.len_utf8();
            }
        }
    }

    fn paint(
        &self,
        origin: Point<Pixels>,
        metrics: TerminalMetrics,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = point(
            origin.x + metrics.cell_width * self.col as f32,
            origin.y + metrics.line_height * self.row as f32,
        );

        window
            .text_system()
            .shape_line(
                self.text.clone().into(),
                self.font_size,
                std::slice::from_ref(&self.style),
                // GPUI interprets this as one glyph's fixed advance, not the
                // aggregate width of the text run.
                Some(metrics.cell_width),
            )
            .paint(
                pos,
                metrics.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
    }
}

#[derive(Clone, Copy)]
struct CursorLayout {
    row: usize,
    col: usize,
    shape: CursorShape,
    color: Hsla,
}

pub struct TerminalElement {
    view: Entity<Ashell>,
    focus_handle: FocusHandle,
    snapshot: RenderSnapshot,
    marked_text: Option<String>,
    font_family: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    cell_width: Pixels,
    tab_id: String,
    search_highlights: Option<std::collections::HashMap<(i32, i32), Hsla>>,
}

pub(crate) struct TerminalElementOptions {
    pub(crate) view: Entity<Ashell>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) snapshot: RenderSnapshot,
    pub(crate) marked_text: Option<String>,
    pub(crate) font_family: SharedString,
    pub(crate) font_size: Pixels,
    pub(crate) line_height: Pixels,
    pub(crate) cell_width: Pixels,
    pub(crate) tab_id: String,
    pub(crate) search_highlights: Option<std::collections::HashMap<(i32, i32), Hsla>>,
}

pub struct PrepaintState {
    bounds: Bounds<Pixels>,
    metrics: TerminalMetrics,
    rects: Vec<LayoutRect>,
    runs: Vec<BatchedTextRun>,
    custom_blocks: Vec<LayoutCustomBlock>,
    cursor: Option<CursorLayout>,
    underlines: Vec<LayoutUnderline>,
}

#[derive(Clone)]
struct LayoutCustomBlock {
    c: char,
    row: i32,
    col: i32,
    cells: usize,
    color: Hsla,
}

struct TerminalInputHandler {
    view: Entity<Ashell>,
    element_bounds: Bounds<Pixels>,
    cell_width: f32,
    line_height: f32,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.view
            .read(cx)
            .terminal_accepts_text_input()
            .then_some(UTF16Selection {
                range: 0..0,
                reversed: false,
            })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.view.read(cx).terminal_marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.commit_terminal_ime_text(text, window, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.set_terminal_marked_text(new_text.to_string(), window, cx);
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.clear_terminal_marked_text(window, cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.view.read(cx).terminal_ime_bounds_for_range(
            range_utf16,
            self.element_bounds,
            self.cell_width,
            self.line_height,
        )
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&mut self, _window: &mut Window, cx: &mut App) -> bool {
        self.view.read(cx).terminal_accepts_text_input()
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, cx: &mut App) -> bool {
        self.view.read(cx).terminal_accepts_text_input()
    }
}

impl TerminalElement {
    pub(crate) fn new(options: TerminalElementOptions) -> Self {
        let TerminalElementOptions {
            view,
            focus_handle,
            snapshot,
            marked_text,
            font_family,
            font_size,
            line_height,
            cell_width,
            tab_id,
            search_highlights,
        } = options;
        Self {
            view,
            focus_handle,
            snapshot,
            marked_text,
            font_family,
            font_size,
            line_height,
            cell_width,
            tab_id,
            search_highlights,
        }
    }

    fn base_text_style(&self, cx: &App) -> TextStyle {
        TextStyle {
            color: cx.theme().foreground,
            font_family: self.font_family.clone(),
            font_size: self.font_size.into(),
            line_height: self.line_height.into(),
            ..Default::default()
        }
    }

    fn cell_run_style(&self, cell: &alacritty_terminal::term::cell::Cell, cx: &App) -> TextRun {
        let mut fg = color_to_hsla(cell.fg, true, cx);
        let mut bg = color_to_hsla(cell.bg, false, cx);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg.a *= 0.7;
        }

        let underline = cell
            .flags
            .intersects(Flags::ALL_UNDERLINES)
            .then(|| UnderlineStyle {
                color: Some(fg),
                thickness: px(1.0),
                wavy: cell.flags.contains(Flags::UNDERCURL),
            });
        let strikethrough = cell
            .flags
            .contains(Flags::STRIKEOUT)
            .then(|| StrikethroughStyle {
                color: Some(fg),
                thickness: px(1.0),
            });

        TextRun {
            len: cell.c.len_utf8(),
            color: fg,
            background_color: None,
            font: Font {
                family: self.font_family.clone(),
                ..Font::default()
            },
            underline,
            strikethrough,
        }
    }

    fn layout_grid(
        &self,
        cx: &App,
    ) -> (
        Vec<LayoutRect>,
        Vec<BatchedTextRun>,
        Vec<LayoutCustomBlock>,
        Vec<LayoutUnderline>,
    ) {
        let view_read = self.view.read(cx);
        let hovered_url = view_read.hovered_url.clone();
        let link_modifier_pressed = view_read.terminal_link_ctrl_pressed;

        let mut rects = Vec::new();
        let mut runs = Vec::new();
        let mut custom_blocks = Vec::new();
        let mut underlines = Vec::new();
        let mut current_run: Option<BatchedTextRun> = None;

        // Retrieve cached keyword highlights and merge with search highlights
        let mut highlights = self.snapshot.highlights.clone();
        if let Some(sm) = self.search_highlights.as_ref() {
            highlights.extend(sm.iter().map(|(k, v)| (*k, *v)));
        }

        for render_cell in &self.snapshot.cells {
            let cell = &render_cell.cell;
            if cell.flags.intersects(
                Flags::HIDDEN | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
            ) {
                continue;
            }

            let selected = self.snapshot.selection.is_some_and(|selection| {
                selection_contains(selection, render_cell.row, render_cell.col)
            });
            let bg = color_to_hsla(cell.bg, false, cx);
            if selected || !is_default_bg(cell.bg) || cell.flags.contains(Flags::INVERSE) {
                rects.push(LayoutRect {
                    row: render_cell.row,
                    col: render_cell.col,
                    cells: terminal_cell_span(cell.flags),
                    color: if selected {
                        cx.theme().selection
                    } else if cell.flags.contains(Flags::INVERSE) {
                        color_to_hsla(cell.fg, true, cx)
                    } else {
                        bg
                    },
                });
            }

            if is_blank(cell) {
                if let Some(run) = current_run.take() {
                    runs.push(run);
                }
                continue;
            }

            let mut style = self.cell_run_style(cell, cx);

            // Apply keyword highlight color if this cell was matched.
            if let Some(&hl_color) = highlights.get(&(render_cell.row, render_cell.col)) {
                style.color = hl_color;
            }

            // Apply hover underline if mouse is hovering over this URL
            if link_modifier_pressed
                && let Some(hu) = &hovered_url
                && hu.tab_id == self.tab_id
                && hu
                    .cells
                    .contains(&(render_cell.row as usize, render_cell.col as usize))
            {
                underlines.push(LayoutUnderline {
                    row: render_cell.row,
                    col: render_cell.col,
                    cells: terminal_cell_span(cell.flags),
                    color: style.color,
                });
            }

            // Box Drawing & Block Elements interception
            let is_custom_block = is_custom_block_supported(cell.c);

            if is_custom_block {
                if let Some(run) = current_run.take() {
                    runs.push(run);
                }
                custom_blocks.push(LayoutCustomBlock {
                    c: cell.c,
                    row: render_cell.row,
                    col: render_cell.col,
                    cells: terminal_cell_span(cell.flags),
                    color: style.color,
                });
                continue;
            }

            if let Some(run) = current_run.as_mut()
                && run.can_append(&style, render_cell.row, render_cell.col)
            {
                run.append(cell.c, cell.zerowidth());
                continue;
            }

            if let Some(run) = current_run.take() {
                runs.push(run);
            }

            let mut run = BatchedTextRun::new(
                render_cell.row,
                render_cell.col,
                cell.c,
                style,
                self.font_size,
            );
            if let Some(chars) = cell.zerowidth() {
                for ch in chars {
                    run.text.push(*ch);
                    run.style.len += ch.len_utf8();
                }
            }
            current_run = Some(run);
        }

        if let Some(run) = current_run {
            runs.push(run);
        }

        (
            merge_rects(rects),
            runs,
            custom_blocks,
            merge_underlines(underlines),
        )
    }

    fn cursor_layout(&self, cx: &App) -> Option<CursorLayout> {
        use crate::session::config::CursorStyle;
        let cursor_style = self.view.read(cx).cursor_style;
        let show_cursor = match cursor_style {
            CursorStyle::Blink | CursorStyle::BeamBlink => {
                if let Ok(duration) =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                {
                    (duration.as_millis() / 600) % 2 == 0
                } else {
                    true
                }
            }
            _ => true,
        };

        self.snapshot.cursor.map(|cursor| {
            let mut shape = match cursor_style {
                CursorStyle::Default => cursor.shape,
                CursorStyle::Blink => CursorShape::Block,
                CursorStyle::Beam => CursorShape::Beam,
                CursorStyle::BeamBlink => CursorShape::Beam,
            };
            if !show_cursor {
                shape = CursorShape::Hidden;
            }
            CursorLayout {
                row: cursor.row,
                col: cursor.col,
                shape,
                color: cx.theme().primary,
            }
        })
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let _ = self.base_text_style(cx);
        let (rects, runs, custom_blocks, underlines) = self.layout_grid(cx);

        // Save the precise GPUI-rendered bounds of this terminal element.
        // This is 100% accurate because it is recorded during layout prepaint.
        let view = self.view.clone();
        let tab_id = self.tab_id.clone();
        view.update(cx, |this, cx| {
            let old_bounds = this.terminal_bounds.insert(tab_id.clone(), bounds);

            // Sync PTY size unconditionally on every prepaint layout pass to ensure
            // absolute synchronization with GPUI layout regardless of intermediate events.
            let line_height = this.terminal_line_height();
            let cell_width = this.terminal_cell_width();
            let w = bounds.size.width.as_f32();
            let h = bounds.size.height.as_f32();
            let cols = (w / cell_width).floor().max(1.0) as u16;
            let rows = (h / line_height).floor().max(1.0) as u16;

            if let Some(tab) = this.tabs.iter_mut().find(|t| t.id == tab_id) {
                tab.resize(cols, rows);
            }

            if old_bounds != Some(bounds) {
                cx.notify();
            }
        });

        PrepaintState {
            bounds,
            metrics: TerminalMetrics {
                cell_width: self.cell_width,
                line_height: self.line_height,
            },
            rects,
            runs,
            custom_blocks,
            cursor: self.cursor_layout(cx),
            underlines,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Compute a vertical offset to center the text grid vertically,
        // distributing the leftover pixel remainder evenly to the top and bottom.
        let grid_height = prepaint.metrics.line_height
            * (prepaint.bounds.size.height.as_f32() / prepaint.metrics.line_height.as_f32())
                .floor();
        let y_offset = ((prepaint.bounds.size.height - grid_height) / 2.0).max(px(0.0));
        let draw_origin = point(
            prepaint.bounds.origin.x,
            prepaint.bounds.origin.y + y_offset,
        );

        for rect in &prepaint.rects {
            rect.paint(draw_origin, prepaint.metrics, window);
        }

        for run in &prepaint.runs {
            run.paint(draw_origin, prepaint.metrics, window, cx);
        }

        for u in &prepaint.underlines {
            u.paint(draw_origin, prepaint.metrics, window);
        }

        for block in &prepaint.custom_blocks {
            let x =
                draw_origin.x.as_f32() + block.col as f32 * prepaint.metrics.cell_width.as_f32();
            let y =
                draw_origin.y.as_f32() + block.row as f32 * prepaint.metrics.line_height.as_f32();
            paint_custom_block(
                window,
                block.c,
                x,
                y,
                prepaint.metrics.cell_width.as_f32() * block.cells as f32,
                prepaint.metrics.line_height.as_f32(),
                block.color,
            );
        }

        window.handle_input(
            &self.focus_handle,
            TerminalInputHandler {
                view: self.view.clone(),
                element_bounds: Bounds::new(draw_origin, prepaint.bounds.size),
                cell_width: prepaint.metrics.cell_width.as_f32(),
                line_height: prepaint.metrics.line_height.as_f32(),
            },
            cx,
        );

        if let Some(marked_text) = self.marked_text.as_ref().filter(|text| !text.is_empty())
            && let Some(cursor) = prepaint.cursor
        {
            let pos = point(
                draw_origin.x + prepaint.metrics.cell_width * cursor.col as f32,
                draw_origin.y + prepaint.metrics.line_height * cursor.row as f32,
            );
            let mut base_style = self.base_text_style(cx);
            base_style.underline = Some(UnderlineStyle {
                color: Some(base_style.color),
                thickness: px(1.0),
                wavy: false,
            });
            let shaped = window.text_system().shape_line(
                marked_text.clone().into(),
                self.font_size,
                &[TextRun {
                    len: marked_text.len(),
                    font: Font {
                        family: self.font_family.clone(),
                        ..Font::default()
                    },
                    color: base_style.color,
                    underline: base_style.underline,
                    ..Default::default()
                }],
                None,
            );
            let bg_bounds =
                Bounds::new(pos, gpui::size(shaped.width, prepaint.metrics.line_height));
            window.paint_quad(fill(bg_bounds, cx.theme().background));
            shaped
                .paint(
                    pos,
                    prepaint.metrics.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }

        if let Some(cursor) = prepaint.cursor {
            if self
                .marked_text
                .as_ref()
                .is_some_and(|text| !text.is_empty())
            {
                return;
            }
            let x = draw_origin.x + prepaint.metrics.cell_width * cursor.col as f32;
            let y = draw_origin.y + prepaint.metrics.line_height * cursor.row as f32;
            match cursor.shape {
                CursorShape::Hidden => {}
                CursorShape::Beam => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x, y),
                            gpui::size(px(2.), prepaint.metrics.line_height),
                        ),
                        cursor.color,
                    ));
                }
                CursorShape::Underline => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x, y + prepaint.metrics.line_height - px(2.)),
                            gpui::size(prepaint.metrics.cell_width, px(2.)),
                        ),
                        cursor.color,
                    ));
                }
                CursorShape::Block | CursorShape::HollowBlock => {
                    let alpha = if matches!(cursor.shape, CursorShape::HollowBlock) {
                        0.18
                    } else {
                        0.32
                    };
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x, y),
                            gpui::size(prepaint.metrics.cell_width, prepaint.metrics.line_height),
                        ),
                        cursor.color.opacity(alpha),
                    ));
                }
            }
        }
    }
}

fn merge_rects(mut rects: Vec<LayoutRect>) -> Vec<LayoutRect> {
    rects.sort_by_key(|rect| (rect.row, rect.col));
    let mut merged: Vec<LayoutRect> = Vec::with_capacity(rects.len());

    for rect in rects {
        if let Some(last) = merged.last_mut()
            && last.row == rect.row
            && last.color == rect.color
            && last.col + last.cells as i32 == rect.col
        {
            last.cells += rect.cells;
            continue;
        }
        merged.push(rect);
    }

    merged
}

fn merge_underlines(mut underlines: Vec<LayoutUnderline>) -> Vec<LayoutUnderline> {
    underlines.sort_by_key(|u| (u.row, u.col));
    let mut merged: Vec<LayoutUnderline> = Vec::with_capacity(underlines.len());

    for u in underlines {
        if let Some(last) = merged.last_mut()
            && last.row == u.row
            && last.color == u.color
            && last.col + last.cells as i32 == u.col
        {
            last.cells += u.cells;
            continue;
        }
        merged.push(u);
    }

    merged
}

fn selection_contains(selection: ViewportSelection, row: i32, col: i32) -> bool {
    let row = row.max(0) as usize;
    let col = col.max(0) as usize;

    if row < selection.start_row || row > selection.end_row {
        return false;
    }

    if selection.is_block {
        return col >= selection.start_col && col <= selection.end_col;
    }

    let after_start = row > selection.start_row || col >= selection.start_col;
    let before_end = row < selection.end_row || col <= selection.end_col;
    after_start && before_end
}

fn is_blank(cell: &alacritty_terminal::term::cell::Cell) -> bool {
    cell.c == ' '
        && cell.zerowidth().is_none()
        && !cell
            .flags
            .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
}

fn is_default_bg(color: AnsiColor) -> bool {
    matches!(color, AnsiColor::Named(NamedColor::Background))
}

fn color_to_hsla(color: AnsiColor, foreground: bool, cx: &App) -> Hsla {
    match color {
        AnsiColor::Spec(rgb) => Hsla::from(Rgba {
            r: rgb.r as f32 / 255.0,
            g: rgb.g as f32 / 255.0,
            b: rgb.b as f32 / 255.0,
            a: 1.0,
        }),
        AnsiColor::Indexed(index) => ansi_index_color(index, cx),
        AnsiColor::Named(named) => named_color(named, foreground, cx),
    }
}

fn ansi_index_color(index: u8, cx: &App) -> Hsla {
    if (index as usize) < TERMINAL_DARK_ANSI_16.len() {
        return Hsla::from(rgb(terminal_ansi_16(index, cx.theme().is_dark())));
    }

    if index >= 232 {
        return Hsla::from(rgb(soften_xterm_gray(index, cx.theme().is_dark())));
    }

    let i = index - 16;
    let r = i / 36;
    let g = (i % 36) / 6;
    let b = i % 6;
    let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
    Hsla::from(rgb(soften_xterm_rgb(
        pack_rgb(conv(r), conv(g), conv(b)),
        cx.theme().is_dark(),
    )))
}

fn named_color(named: NamedColor, _foreground: bool, cx: &App) -> Hsla {
    match named {
        NamedColor::Foreground => cx.theme().foreground,
        NamedColor::Background => cx.theme().background,
        NamedColor::Black => Hsla::from(rgb(terminal_ansi_16(0, cx.theme().is_dark()))),
        NamedColor::Red => Hsla::from(rgb(terminal_ansi_16(1, cx.theme().is_dark()))),
        NamedColor::Green => Hsla::from(rgb(terminal_ansi_16(2, cx.theme().is_dark()))),
        NamedColor::Yellow => Hsla::from(rgb(terminal_ansi_16(3, cx.theme().is_dark()))),
        NamedColor::Blue => Hsla::from(rgb(terminal_ansi_16(4, cx.theme().is_dark()))),
        NamedColor::Magenta => Hsla::from(rgb(terminal_ansi_16(5, cx.theme().is_dark()))),
        NamedColor::Cyan => Hsla::from(rgb(terminal_ansi_16(6, cx.theme().is_dark()))),
        NamedColor::White => Hsla::from(rgb(terminal_ansi_16(7, cx.theme().is_dark()))),
        NamedColor::BrightBlack => Hsla::from(rgb(terminal_ansi_16(8, cx.theme().is_dark()))),
        NamedColor::BrightRed => Hsla::from(rgb(terminal_ansi_16(9, cx.theme().is_dark()))),
        NamedColor::BrightGreen => Hsla::from(rgb(terminal_ansi_16(10, cx.theme().is_dark()))),
        NamedColor::BrightYellow => Hsla::from(rgb(terminal_ansi_16(11, cx.theme().is_dark()))),
        NamedColor::BrightBlue => Hsla::from(rgb(terminal_ansi_16(12, cx.theme().is_dark()))),
        NamedColor::BrightMagenta => Hsla::from(rgb(terminal_ansi_16(13, cx.theme().is_dark()))),
        NamedColor::BrightCyan => Hsla::from(rgb(terminal_ansi_16(14, cx.theme().is_dark()))),
        NamedColor::BrightWhite => Hsla::from(rgb(terminal_ansi_16(15, cx.theme().is_dark()))),
        NamedColor::Cursor => cx.theme().primary,
        NamedColor::DimForeground => cx.theme().muted_foreground,
        NamedColor::BrightForeground => cx.theme().foreground,
        NamedColor::DimBlack => Hsla::from(rgb(dim_terminal_ansi_16(0, cx.theme().is_dark()))),
        NamedColor::DimRed => Hsla::from(rgb(dim_terminal_ansi_16(1, cx.theme().is_dark()))),
        NamedColor::DimGreen => Hsla::from(rgb(dim_terminal_ansi_16(2, cx.theme().is_dark()))),
        NamedColor::DimYellow => Hsla::from(rgb(dim_terminal_ansi_16(3, cx.theme().is_dark()))),
        NamedColor::DimBlue => Hsla::from(rgb(dim_terminal_ansi_16(4, cx.theme().is_dark()))),
        NamedColor::DimMagenta => Hsla::from(rgb(dim_terminal_ansi_16(5, cx.theme().is_dark()))),
        NamedColor::DimCyan => Hsla::from(rgb(dim_terminal_ansi_16(6, cx.theme().is_dark()))),
        NamedColor::DimWhite => Hsla::from(rgb(dim_terminal_ansi_16(7, cx.theme().is_dark()))),
    }
}

const TERMINAL_DARK_ANSI_16: [u32; 16] = [
    0x191919, 0xd76e6e, 0x8fbd8b, 0xd2b86e, 0x8eace2, 0xbf8fcb, 0x82c8c0, 0xd8d8d8, 0x777777,
    0xe48989, 0xa4d19f, 0xdfc885, 0xa4bff0, 0xd1a4dc, 0x99d8d1, 0xf3f3f3,
];

const TERMINAL_LIGHT_ANSI_16: [u32; 16] = [
    0x242424, 0xa84444, 0x3d7a4b, 0x80651d, 0x315f99, 0x754b82, 0x2f7770, 0xeeeeee, 0x767676,
    0xbf5b5b, 0x4f8d5c, 0x96762a, 0x4773ad, 0x8a5d98, 0x428b83, 0xffffff,
];

fn terminal_ansi_16(index: u8, is_dark: bool) -> u32 {
    let palette = if is_dark {
        TERMINAL_DARK_ANSI_16
    } else {
        TERMINAL_LIGHT_ANSI_16
    };
    palette[index as usize]
}

fn dim_terminal_ansi_16(index: u8, is_dark: bool) -> u32 {
    let color = terminal_ansi_16(index, is_dark);
    let anchor = if is_dark { 0x8a8a8a } else { 0x5f5f5f };
    mix_rgb(color, anchor, 0.42)
}

fn soften_xterm_gray(index: u8, is_dark: bool) -> u32 {
    let gray = 8 + (index - 232) * 10;
    let adjusted = if is_dark {
        gray.clamp(72, 218)
    } else {
        gray.clamp(42, 178)
    };
    pack_rgb(adjusted, adjusted, adjusted)
}

fn soften_xterm_rgb(color: u32, is_dark: bool) -> u32 {
    let (r, g, b) = unpack_rgb(color);
    let gray = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as u8;
    let neutral = pack_rgb(gray, gray, gray);
    let softened = mix_rgb(color, neutral, 0.38);
    if is_dark {
        mix_rgb(softened, 0xd0d0d0, 0.10)
    } else {
        mix_rgb(softened, 0x222222, 0.18)
    }
}

fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn unpack_rgb(color: u32) -> (u8, u8, u8) {
    (
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

fn mix_rgb(color: u32, target: u32, amount: f32) -> u32 {
    let (r, g, b) = unpack_rgb(color);
    let (tr, tg, tb) = unpack_rgb(target);
    let mix = |from: u8, to: u8| {
        (from as f32 + (to as f32 - from as f32) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    pack_rgb(mix(r, tr), mix(g, tg), mix(b, tb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_cell_span_matches_terminal_column_width() {
        assert_eq!(terminal_cell_span(Flags::empty()), 1);
        assert_eq!(terminal_cell_span(Flags::WIDE_CHAR), 2);
    }
}
