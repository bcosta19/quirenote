// "breakindent" (writing.ts): when a list item or a quote wraps, the continuation aligns with
// the item text, not the margin. egui wraps every row at the left edge, so the editor lays out
// each paragraph itself: the first row as usual, the rest narrower and shifted right by the
// width of the prefix (measured in the paragraph's own font), then joins the pieces.

use std::sync::{Arc, LazyLock};

use eframe::egui::{
    Galley, Ui,
    text::{LayoutJob, LayoutSection},
};
use regex::Regex;

/// Line prefix that sets the continuation indent: spaces + list/quote marker.
static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ \t]*)([-*+][ \t]+\[[ xX]\][ \t]+|[-*+][ \t]+|\d+[.)][ \t]+|>[ \t]*)?").unwrap_or_else(|_| unreachable!())
});

/// The part of `job` in the byte range, as a job of its own (sections clipped and rebased).
fn slice(job: &LayoutJob, start: usize, end: usize, wrap: f32) -> LayoutJob {
    let mut out = LayoutJob {
        text: job.text[start..end].to_owned(),
        sections: Vec::new(),
        break_on_newline: job.break_on_newline,
        halign: job.halign,
        justify: job.justify,
        round_output_to_gui: job.round_output_to_gui,
        keep_trailing_whitespace: job.keep_trailing_whitespace,
        ..Default::default()
    };
    out.wrap = job.wrap.clone();
    out.wrap.max_width = wrap;
    // Like epaint's own paragraph split: a section ending where the slice starts is behind it;
    // one starting where it ends gives an empty slice its format.
    for s in &job.sections {
        let (a, b) = (s.byte_range.start.0, s.byte_range.end.0);
        if b <= start {
            continue;
        }
        if end < a {
            break;
        }
        let range = (a.max(start) - start).into()..(b.min(end) - start).into();
        out.sections.push(LayoutSection { leading_space: 0.0, byte_range: range, format: s.format.clone() });
    }
    out
}

/// Lays the job out with hanging indents for wrapped list items and quotes.
pub fn layout(ui: &Ui, job: &LayoutJob, wrap: f32) -> Arc<Galley> {
    let text = &job.text;
    let mut parts: Vec<Arc<Galley>> = Vec::new();
    // Indexes (in `parts`) of pieces that continue on the next piece without a line break.
    let mut joined = Vec::new();
    let mut start = 0;
    loop {
        let mut end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        if end + 1 == text.len() && text.ends_with('\n') {
            end += 1; // a trailing newline stays in the last paragraph
        }
        let para = slice(job, start, end, wrap);
        let g = ui.fonts_mut(|f| f.layout_job(para.clone()));
        let prefix_len = PREFIX.find(&para.text).map_or(0, |m| m.end());
        if g.rows.len() > 1 && prefix_len > 0 && prefix_len < para.text.trim_end_matches('\n').len() {
            let measure = slice(&para, 0, prefix_len, f32::INFINITY);
            let indent = ui.fonts_mut(|f| f.layout_job(measure)).size().x;
            let first_chars = g.rows[0].row.char_count_excluding_newline().0;
            let split = para.text.char_indices().nth(first_chars).map_or(para.text.len(), |(b, _)| b);
            if indent > 0.0 && indent < wrap * 0.6 && split > 0 && split < para.text.len() {
                let first = ui.fonts_mut(|f| f.layout_job(slice(&para, 0, split, wrap)));
                let rest = ui.fonts_mut(|f| f.layout_job(slice(&para, split, para.text.len(), wrap - indent)));
                let mut shifted = (*rest).clone();
                for r in &mut shifted.rows {
                    r.pos.x += indent;
                }
                shifted.rect = shifted.rect.translate(eframe::egui::vec2(indent, 0.0));
                shifted.mesh_bounds = shifted.mesh_bounds.translate(eframe::egui::vec2(indent, 0.0));
                joined.push(parts.len());
                parts.push(first);
                parts.push(Arc::new(shifted));
            } else {
                parts.push(g);
            }
        } else {
            parts.push(g);
        }
        if end >= text.len() {
            break;
        }
        start = end + 1;
        if start == text.len() {
            // Text ending with "\n": an empty last paragraph.
            parts.push(ui.fonts_mut(|f| f.layout_job(slice(job, start, start, wrap))));
            break;
        }
    }
    let ppp = ui.ctx().pixels_per_point();
    let mut g = Galley::concat(Arc::new(job.clone()), &parts, ppp);
    // concat puts a line break between pieces: the pieces of one paragraph have none.
    let mut row = 0;
    for (i, p) in parts.iter().enumerate() {
        row += p.rows.len();
        if joined.contains(&i) {
            if let Some(r) = g.rows.get_mut(row - 1) {
                r.ends_with_newline = false;
            }
        }
    }
    Arc::new(g)
}
