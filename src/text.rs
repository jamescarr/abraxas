//! Text helpers: CP437, pipe codes, wrapping, placeholders (extended by the foundations step).

/// The Shadow's handle: the player's handle reversed with ASCII case swapped, plus `_` when the
/// result would equal the original.
pub fn shadow_handle(handle: &str) -> String {
    let swapped: String = handle
        .chars()
        .rev()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    if swapped == handle {
        format!("{swapped}_")
    } else {
        swapped
    }
}
