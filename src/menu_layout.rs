/// The 20 greatest-radius LEDs, sorted around the circumference and split at
/// the five larger angular gaps. Each group is four physically adjacent outer
/// pixels; group order follows the perimeter.
pub const GROUPS: [[usize; 4]; 5] = [
    [73, 72, 64, 63],
    [55, 54, 46, 45],
    [37, 36, 28, 27],
    [19, 18, 10, 9],
    [1, 0, 82, 81],
];

pub fn is_reserved(index: usize) -> bool {
    GROUPS.iter().flatten().any(|reserved| *reserved == index)
}

/// Position of a physical group relative to the moving selected group, in the
/// range -2..=2 while walking forward around the five-group perimeter.
pub const fn relative_to_selected(position: usize, selected: usize) -> i32 {
    match (position + GROUPS.len() - selected) % GROUPS.len() {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => -2,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_menu_uses_twenty_distinct_leds() {
        let mut seen = [false; 90];
        let mut count = 0;
        for index in GROUPS.iter().flatten().copied() {
            assert!(index < seen.len());
            assert!(!seen[index]);
            seen[index] = true;
            count += 1;
        }
        assert_eq!(count, 20);
    }

    #[test]
    fn groups_are_four_adjacent_outer_pixels() {
        assert_eq!(GROUPS[0], [73, 72, 64, 63]);
        assert_eq!(GROUPS[1], [55, 54, 46, 45]);
        assert_eq!(GROUPS[2], [37, 36, 28, 27]);
        assert_eq!(GROUPS[3], [19, 18, 10, 9]);
        assert_eq!(GROUPS[4], [1, 0, 82, 81]);
    }

    #[test]
    fn selected_group_moves_and_neighbors_remain_relative() {
        for selected in 0..GROUPS.len() {
            let offsets = core::array::from_fn::<_, 5, _>(|position| {
                relative_to_selected(position, selected)
            });
            assert!(offsets.contains(&0));
            assert!(offsets.contains(&-1));
            assert!(offsets.contains(&1));
            assert!(offsets.contains(&-2));
            assert!(offsets.contains(&2));
            assert_eq!(relative_to_selected(selected, selected), 0);
        }
    }
}
