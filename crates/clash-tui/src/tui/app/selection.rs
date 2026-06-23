use ratatui::layout::Rect;

use crate::tui::views;

pub(crate) fn visible_indices(indices: &[usize], selected: usize, max_rows: usize) -> &[usize] {
    visible_indices_with_offset(indices, selected, max_rows).0
}

pub(crate) fn visible_indices_with_offset(indices: &[usize], selected: usize, max_rows: usize) -> (&[usize], usize) {
    if indices.len() <= max_rows {
        return (indices, 0);
    }
    let selected_pos = indices.iter().position(|index| *index == selected).unwrap_or(0);
    let start = selected_pos
        .saturating_sub(max_rows / 2)
        .min(indices.len().saturating_sub(max_rows));
    (&indices[start..start + max_rows], start)
}

pub(crate) fn content_rows(area: Rect, reserved: u16) -> usize {
    usize::from(area.height.saturating_sub(2 + reserved)).max(1)
}

pub(crate) fn filter_indices(count: usize, mut predicate: impl FnMut(usize) -> bool) -> Vec<usize> {
    (0..count).filter(|index| predicate(*index)).collect()
}

pub(crate) fn text_matches<'a>(query: &str, values: impl IntoIterator<Item = Option<&'a str>>) -> bool {
    let query = query.to_ascii_lowercase();
    values
        .into_iter()
        .flatten()
        .any(|value| value.to_ascii_lowercase().contains(&query))
}

pub(crate) fn proxy_selection_key_matches(current: &str, remembered: &str) -> bool {
    if current == remembered {
        return true;
    }
    let current = views::layout::stable_table_text(current);
    let remembered = views::layout::stable_table_text(remembered);
    current != "-" && current == remembered
}

pub(crate) fn move_index(index: &mut usize, len: usize, delta: isize) {
    if len == 0 {
        *index = 0;
        return;
    }
    if delta == isize::MIN {
        *index = 0;
        return;
    }
    if delta == isize::MAX {
        *index = len - 1;
        return;
    }
    let current = isize::try_from(*index).unwrap_or(0);
    let last = isize::try_from(len.saturating_sub(1)).unwrap_or(0);
    *index = current.saturating_add(delta).clamp(0, last) as usize;
}

pub(crate) fn move_in_indices(selected: &mut usize, indices: &[usize], delta: isize) {
    if indices.is_empty() {
        *selected = 0;
        return;
    }
    let position = indices.iter().position(|index| *index == *selected).unwrap_or(0);
    let mut next = position;
    move_index(&mut next, indices.len(), delta);
    *selected = indices[next];
}

pub(crate) const fn clamp_index(index: &mut usize, len: usize) {
    if len == 0 {
        *index = 0;
    } else if *index >= len {
        *index = len - 1;
    }
}

pub(crate) fn clamp_with_indices(selected: &mut usize, indices: &[usize]) {
    if indices.is_empty() {
        *selected = 0;
    } else if !indices.contains(selected) {
        *selected = indices[0];
    }
}
