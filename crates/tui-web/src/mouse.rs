//! Pointer positions and wheel turns in the page as the TUI's mouse input.

/// Pixels of wheel movement that make one scroll step; trackpads send many
/// small deltas, mouse wheels about 100 a notch.
const STEP_PX: f64 = 50.0;

/// Pixels in a wheel line when the browser counts in lines (Firefox).
const LINE_PX: f64 = 16.0;

/// A box on the page, in CSS pixels as `getBoundingClientRect` gives it,
/// scaling included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// The cell under the point (`x`, `y`) of a grid of `columns` x `rows`
/// cells drawn in `bounds`; `None` outside it.
pub fn cell(x: f64, y: f64, bounds: Bounds, columns: u16, rows: u16) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let column = ((x - bounds.left) / bounds.width * f64::from(columns)).floor();
    let row = ((y - bounds.top) / bounds.height * f64::from(rows)).floor();
    let inside = |value: f64, count: u16| value >= 0.0 && value < f64::from(count);
    (inside(column, columns) && inside(row, rows)).then_some((column as u16, row as u16))
}

/// Wheel movement not yet worth a scroll step.
#[derive(Debug, Default)]
pub struct Wheel {
    pending: f64,
}

impl Wheel {
    /// Adds a wheel event's vertical delta (`deltaMode` 1 counts lines, 2
    /// pages of `page_px`) and returns the whole steps it completes:
    /// positive scrolls down, negative up.
    pub fn steps(&mut self, delta: f64, mode: u32, page_px: f64) -> i32 {
        let px = match mode {
            1 => delta * LINE_PX,
            2 => delta * page_px,
            _ => delta,
        };
        // A turn the other way drops what was left of the last one.
        if px.signum() != self.pending.signum() {
            self.pending = 0.0;
        }
        self.pending += px;
        let steps = (self.pending / STEP_PX).trunc();
        self.pending -= steps * STEP_PX;
        steps as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: Bounds = Bounds {
        left: 100.0,
        top: 50.0,
        width: 960.0,
        height: 680.0,
    };

    #[test]
    fn points_map_to_cells_and_the_edges_stay_inside() {
        assert_eq!(cell(100.0, 50.0, GRID, 120, 40), Some((0, 0)));
        assert_eq!(cell(108.5, 67.5, GRID, 120, 40), Some((1, 1)));
        assert_eq!(cell(1059.9, 729.9, GRID, 120, 40), Some((119, 39)));
        assert_eq!(cell(1060.0, 60.0, GRID, 120, 40), None);
        assert_eq!(cell(99.0, 60.0, GRID, 120, 40), None);
    }

    #[test]
    fn a_scaled_grid_maps_the_same_cells() {
        let half = Bounds {
            width: GRID.width / 2.0,
            height: GRID.height / 2.0,
            ..GRID
        };
        assert_eq!(cell(100.0 + 4.25, 50.0 + 8.75, half, 120, 40), Some((1, 1)));
    }

    #[test]
    fn small_deltas_add_up_and_a_turn_back_starts_over() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.steps(30.0, 0, 600.0), 0);
        assert_eq!(wheel.steps(30.0, 0, 600.0), 1);
        assert_eq!(wheel.steps(-30.0, 0, 600.0), 0);
        assert_eq!(wheel.steps(-30.0, 0, 600.0), -1);
        assert_eq!(wheel.steps(3.0, 1, 600.0), 0);
        assert_eq!(wheel.steps(120.0, 0, 600.0), 3);
    }
}
