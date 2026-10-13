// Lucide icons (the same set as @lucide/svelte), drawn from a subset of the Lucide icon font.

pub const FILE_PLUS: char = '\u{e0c9}';
pub const FOLDER_PLUS: char = '\u{e0d9}';
pub const FOLDER_OPEN: char = '\u{e247}';
pub const PANEL_LEFT: char = '\u{e12a}';
pub const SEARCH: char = '\u{e151}';
pub const CORNER_UP_LEFT: char = '\u{e0a7}';
pub const PENCIL: char = '\u{e1f9}';
pub const EYE: char = '\u{e0ba}';
pub const LINK: char = '\u{e102}';
pub const PANEL_LEFT_CLOSE: char = '\u{e21c}';
pub const PANEL_LEFT_OPEN: char = '\u{e21d}';
pub const PANEL_RIGHT_CLOSE: char = '\u{e432}';
pub const PANEL_RIGHT_OPEN: char = '\u{e434}';
pub const X: char = '\u{e1b2}';
pub const MOON: char = '\u{e11e}';
pub const SUN: char = '\u{e178}';
pub const TYPE: char = '\u{e198}';
pub const CHEVRON_DOWN: char = '\u{e06d}';
pub const CHEVRON_RIGHT: char = '\u{e06f}';
pub const CHEVRON_LEFT: char = '\u{e06e}';
pub const BOOK_OPEN: char = '\u{e05f}';
pub const BOOK_TEXT: char = '\u{e54b}';
pub const FILE_TEXT: char = '\u{e0cc}';
pub const MINUS: char = '\u{e11c}';
pub const PLUS: char = '\u{e13d}';
pub const MOVE_HORIZONTAL: char = '\u{e1c6}';
pub const ROTATE_CCW: char = '\u{e148}';

/// Icon by file type: PDF, EPUB or note (FileIcon.svelte).
pub fn file_icon(name: &str) -> char {
    if crate::links::is_pdf(name) {
        BOOK_OPEN
    } else if crate::links::is_epub(name) {
        BOOK_TEXT
    } else {
        FILE_TEXT
    }
}
