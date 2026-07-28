pub(crate) const DEFAULT_COLS: u16 = 100;
pub(crate) const DEFAULT_ROWS: u16 = 30;
pub(crate) const SIDEBAR_WIDTH: f32 = 220.0;
pub(crate) const COLLAPSED_SIDEBAR_WIDTH: f32 = 40.0;
pub(crate) const COMPACT_ICON_SIZE: f32 = 14.0;
pub(crate) const SIDEBAR_SECTION_HEIGHT: f32 = 30.0;
pub(crate) const SIDEBAR_PRIMARY_ACTION_HEIGHT: f32 = 32.0;
pub(crate) const SFTP_TOOLBAR_HEIGHT: f32 = 32.0;
pub(crate) const SFTP_STATUS_HEIGHT: f32 = 26.0;

pub(crate) const TAB_BAR_HEIGHT: f32 = 40.0;
pub(crate) const TERMINAL_PADDING_X: f32 = 22.0;
pub(crate) const TERMINAL_PADDING_Y: f32 = 18.0;
pub(crate) const TERMINAL_FALLBACK_CELL_WIDTH_EM: f32 = 9.75 / 16.0;
pub(crate) const TERMINAL_LINE_HEIGHT_EM: f32 = 22.0 / 16.0;
pub(crate) const TERMINAL_SCROLLBAR_GUTTER: f32 = 16.0;

pub(crate) fn terminal_cell_width_from_measurement(measured: f32, font_size: f32) -> f32 {
    let fallback = (font_size * TERMINAL_FALLBACK_CELL_WIDTH_EM).max(6.0);
    let minimum = font_size * 0.4;
    let maximum = font_size;
    if measured.is_finite() && measured >= minimum && measured <= maximum {
        measured.max(6.0)
    } else {
        fallback
    }
}

pub(crate) const TERMINAL_KEY_CONTEXT: &str = "AshellTerminal";
pub(crate) const DOCUMENT_KEY_CONTEXT: &str = "AshellDocument";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_rail_width_matches_the_preview_layout() {
        assert_eq!(COLLAPSED_SIDEBAR_WIDTH, 40.0);
        assert_eq!(SIDEBAR_WIDTH, 220.0);
        assert_eq!(COMPACT_ICON_SIZE, 14.0);
        assert_eq!(SIDEBAR_SECTION_HEIGHT, 30.0);
        assert_eq!(SIDEBAR_PRIMARY_ACTION_HEIGHT, 32.0);
        assert_eq!(SFTP_TOOLBAR_HEIGHT, 32.0);
        assert_eq!(SFTP_STATUS_HEIGHT, 26.0);
    }

    #[test]
    fn terminal_metrics_match_the_balanced_reference() {
        assert_eq!(terminal_cell_width_from_measurement(9.6, 16.0), 9.6);
        assert_eq!(terminal_cell_width_from_measurement(f32::NAN, 16.0), 9.75);
        assert_eq!(TERMINAL_LINE_HEIGHT_EM, 22.0 / 16.0);
        assert_eq!(TERMINAL_SCROLLBAR_GUTTER, 16.0);
    }
}
