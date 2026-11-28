//! The colour grid test windows draw, so a check can tell a real picture of
//! the window from a blank or garbled one at any size.

use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{Point, Size};

/// Columns of the grid.
pub const GRID_COLUMNS: u32 = 4;
/// Rows of the grid.
pub const GRID_ROWS: u32 = 3;

/// The colour of each cell as RGB, row by row from the top left. Far apart
/// from each other, so a lossy codec cannot turn one into another.
pub const GRID_COLOURS: [[u8; 3]; (GRID_COLUMNS * GRID_ROWS) as usize] = [
    [0xe6, 0x19, 0x4b],
    [0x3c, 0xb4, 0x4b],
    [0xff, 0xe1, 0x19],
    [0x43, 0x63, 0xd8],
    [0xf5, 0x82, 0x31],
    [0x91, 0x1e, 0xb4],
    [0x42, 0xd4, 0xf4],
    [0xf0, 0x32, 0xe6],
    [0xbf, 0xef, 0x45],
    [0x46, 0x99, 0x90],
    [0x9a, 0x63, 0x24],
    [0x00, 0x00, 0x75],
];

/// How far a channel may be off and still count, for lossy codecs.
const TOLERANCE: u8 = 32;

/// The RGB colour of pixel `x`, `y` of a grid drawn at `size`: cells split
/// the picture evenly, rounding down.
pub fn grid_colour(x: u32, y: u32, size: Size) -> [u8; 3] {
    let column = (u64::from(x) * u64::from(GRID_COLUMNS) / u64::from(size.width.max(1))) as u32;
    let row = (u64::from(y) * u64::from(GRID_ROWS) / u64::from(size.height.max(1))) as u32;
    let index = row.min(GRID_ROWS - 1) * GRID_COLUMNS + column.min(GRID_COLUMNS - 1);
    GRID_COLOURS[index as usize]
}

/// The centre of the cell at `column`, `row` of a grid drawn at `size`.
pub fn cell_centre(column: u32, row: u32, size: Size) -> Point {
    let x = (2 * column + 1) * size.width / (2 * GRID_COLUMNS);
    let y = (2 * row + 1) * size.height / (2 * GRID_ROWS);
    Point::new(x as i32, y as i32)
}

/// Packed BGRA rows of the grid at `size`.
pub fn draw_grid(size: Size) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size.width as usize * size.height as usize * 4);
    for y in 0..size.height {
        for x in 0..size.width {
            let [r, g, b] = grid_colour(x, y, size);
            pixels.extend_from_slice(&[b, g, r, 0xff]);
        }
    }
    pixels
}

/// Whether `frame` shows the grid: the centre of every cell has its colour.
pub fn shows_grid(frame: &Frame) -> bool {
    let size = Size::new(frame.width(), frame.height());
    (0..GRID_ROWS).all(|row| {
        (0..GRID_COLUMNS).all(|column| {
            let centre = cell_centre(column, row, size);
            let Some(pixels) = frame.row(centre.y as u32) else {
                return false;
            };
            let at = centre.x as usize * 4;
            let Some(&[b, g, r, _]) = pixels.get(at..at + 4) else {
                return false;
            };
            let close = |got: u8, want: u8| got.abs_diff(want) <= TOLERANCE;
            let [wr, wg, wb] = grid_colour(centre.x as u32, centre.y as u32, size);
            close(r, wr) && close(g, wg) && close(b, wb)
        })
    })
}
