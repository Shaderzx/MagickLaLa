use crate::ChunkConfig;

const SEPARATORS: [&str; 7] = ["\n\n", "\n", ". ", "? ", "! ", "; ", " "];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub char_start: usize,
    pub char_end: usize,
}

#[derive(Clone, Copy)]
struct Piece {
    start: usize,
    end: usize,
    chars: usize,
}

pub fn chunk_text(text: &str, config: ChunkConfig) -> Vec<TextSpan> {
    let max = config.max_chars.max(1);
    let overlap = config.overlap_chars.min(max.saturating_sub(1));
    let mut pieces = Vec::new();
    split_into_pieces(text, 0, text.len(), 0, max, &mut pieces);
    merge_pieces(text, &pieces, max, overlap)
}

fn split_into_pieces(text: &str, start: usize, end: usize, level: usize, max: usize, out: &mut Vec<Piece>) {
    let slice = &text[start..end];
    let chars = slice.chars().count();
    if chars <= max {
        if chars > 0 {
            out.push(Piece { start, end, chars });
        }
        return;
    }
    let Some(separator) = SEPARATORS.get(level) else {
        hard_split(text, start, end, max, out);
        return;
    };
    let mut piece_start = start;
    for (offset, matched) in slice.match_indices(separator) {
        let piece_end = start + offset + matched.len();
        if piece_end > piece_start {
            split_into_pieces(text, piece_start, piece_end, level + 1, max, out);
        }
        piece_start = piece_end;
    }
    if piece_start < end {
        split_into_pieces(text, piece_start, end, level + 1, max, out);
    }
}

fn hard_split(text: &str, start: usize, end: usize, max: usize, out: &mut Vec<Piece>) {
    let mut piece_start = start;
    let mut count = 0;
    for (offset, _) in text[start..end].char_indices() {
        if count == max {
            out.push(Piece { start: piece_start, end: start + offset, chars: count });
            piece_start = start + offset;
            count = 0;
        }
        count += 1;
    }
    if count > 0 {
        out.push(Piece { start: piece_start, end, chars: count });
    }
}

fn merge_pieces(text: &str, pieces: &[Piece], max: usize, overlap: usize) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    let mut window: Vec<Piece> = Vec::new();
    let mut window_chars = 0;
    for piece in pieces {
        if window_chars + piece.chars > max && !window.is_empty() {
            push_span(text, &window, &mut spans);
            let mut carried: Vec<Piece> = Vec::new();
            let mut carried_chars = 0;
            for previous in window.iter().rev() {
                if carried_chars + previous.chars > overlap {
                    break;
                }
                carried_chars += previous.chars;
                carried.push(*previous);
            }
            carried.reverse();
            while !carried.is_empty() && carried_chars + piece.chars > max {
                carried_chars -= carried[0].chars;
                carried.remove(0);
            }
            window = carried;
            window_chars = carried_chars;
        }
        window.push(*piece);
        window_chars += piece.chars;
    }
    push_span(text, &window, &mut spans);
    spans
}

fn push_span(text: &str, window: &[Piece], spans: &mut Vec<TextSpan>) {
    let (Some(first), Some(last)) = (window.first(), window.last()) else {
        return;
    };
    let raw = &text[first.start..last.end];
    let leading = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let byte_start = first.start + leading;
    let byte_end = byte_start + trimmed.len();
    let char_start = text[..byte_start].chars().count();
    let span = TextSpan { byte_start, byte_end, char_start, char_end: char_start + trimmed.chars().count() };
    if spans.last().is_some_and(|previous: &TextSpan| previous.byte_start <= span.byte_start && previous.byte_end >= span.byte_end) {
        return;
    }
    spans.push(span);
}
