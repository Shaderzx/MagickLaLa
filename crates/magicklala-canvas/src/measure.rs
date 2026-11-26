use std::sync::OnceLock;

pub type TextWidth = fn(&str, f32, f32) -> f32;

static MEASURER: OnceLock<TextWidth> = OnceLock::new();

pub fn use_text_width(measure: TextWidth) {
    let _ = MEASURER.set(measure);
}

pub(crate) fn text_width(text: &str, size: f32, weight: f32) -> f32 {
    MEASURER.get().map_or_else(|| text.chars().count() as f32 * 0.6 * size, |measure| measure(text, size, weight))
}

pub(crate) fn wrapped_lines(text: &str, size: f32, weight: f32, room: f32) -> f32 {
    let room = room.max(1.0);
    let space = text_width(" ", size, weight);
    let mut lines = 1.0_f32;
    let mut used = 0.0_f32;
    for word in text.split_whitespace() {
        let wide = text_width(word, size, weight);
        let need = if used > 0.0 { used + space + wide } else { wide };
        if need <= room {
            used = need;
            continue;
        }
        if used > 0.0 {
            lines += 1.0;
        }
        let pieces = (wide / room).ceil().max(1.0);
        lines += pieces - 1.0;
        used = wide - (pieces - 1.0) * room;
    }
    lines
}
