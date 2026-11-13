/// The 20 greatest-radius LEDs of the 89-LED golden-angle board, sorted by
/// angle and cut into five runs of four. This board has no 5-fold symmetry, so
/// the runs are simply consecutive outer pixels (each spans about 55 degrees);
/// the split starts after the one wide gap in the outer ring. Group order
/// follows the perimeter counterclockwise (increasing atan2 angle).
pub const GROUPS: [[usize; 4]; 5] = [
    [76, 83, 84, 0],
    [7, 8, 15, 16],
    [24, 25, 33, 34],
    [41, 42, 49, 50],
    [58, 59, 67, 68],
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
        let mut seen = [false; crate::util::NUM_LEDS];
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
        assert_eq!(GROUPS[0], [76, 83, 84, 0]);
        assert_eq!(GROUPS[1], [7, 8, 15, 16]);
        assert_eq!(GROUPS[2], [24, 25, 33, 34]);
        assert_eq!(GROUPS[3], [41, 42, 49, 50]);
        assert_eq!(GROUPS[4], [58, 59, 67, 68]);
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
