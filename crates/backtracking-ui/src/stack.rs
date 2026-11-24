//! How the waveform rows share the plot's height.
//!
//! A mono or stereo lane keeps its one or two rows. A lane wider than two channels draws one
//! row per channel of the group on show. All rows on screen are the same height. When they
//! cannot all have at least `MIN_ROW_HEIGHT`, the mono and stereo rows keep their place, each
//! wide lane shows as many of its rows as fit (in proportion to its size, at least one) and
//! the rest are reached by scrolling that lane. A plot of many lanes can instead scroll all
//! of its rows as one column (`lay_out_column`).

use std::ops::Range;

/// The lowest row on which a waveform is still readable, in points.
pub const MIN_ROW_HEIGHT: f32 = 14.0;
/// The row height a window grows for when a wide lane appears.
pub const COMFORTABLE_ROW_HEIGHT: f32 = 20.0;
/// The plot's left margin while any lane is stacked: room for labels like "LFE" or "Ltf".
pub const LABEL_GUTTER: f32 = 28.0;
/// Rows lower than this are drawn without their labels, which would overlap.
pub const LABELLED_ROW_HEIGHT: f32 = 9.0;
/// The shortest scroll thumb, so it stays easy to grab.
const MIN_THUMB: f32 = 10.0;

/// The rows a lane asks the layout to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneRows {
    /// The number of rows before layout and scrolling.
    pub rows: usize,
    /// One row per channel. Only such a lane scrolls.
    pub stacked: bool,
}

/// Where one lane's rows go, relative to the top of the plot's audio area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneBox {
    /// The lane's top in points, relative to the plot's audio area.
    pub top: f32,
    /// The lane's visible height in points.
    pub height: f32,
    /// The total number of rows in the lane.
    pub rows: usize,
    /// The first row on screen; the ones above are scrolled out.
    pub first: usize,
    /// The number of rows visible in the lane.
    pub shown: usize,
    /// The height of each visible row in points.
    pub row_height: f32,
}

impl LaneBox {
    /// Whether some rows can only be reached by scrolling.
    pub fn scrolls(&self) -> bool {
        self.shown < self.rows
    }

    /// The range of lane-relative row indices on screen.
    pub fn visible(&self) -> Range<usize> {
        self.first..self.first + self.shown
    }

    /// The top of a row on screen, or `None` while it is scrolled out.
    pub fn row_top(&self, row: usize) -> Option<f32> {
        self.visible()
            .contains(&row)
            .then(|| self.top + (row - self.first) as f32 * self.row_height)
    }

    /// The scroll thumb's top and bottom, for a track as high as the lane.
    pub fn thumb(&self) -> Range<f32> {
        if !self.scrolls() {
            return self.top..self.top + self.height;
        }
        let length = (self.height * self.shown as f32 / self.rows as f32)
            .max(MIN_THUMB)
            .min(self.height);
        let travel = self.height - length;
        let start = self.top + travel * self.first as f32 / (self.rows - self.shown) as f32;
        start..start + length
    }

    /// The first row that puts the thumb's top nearest `thumb_top`.
    pub fn first_for_thumb(&self, thumb_top: f32) -> usize {
        let thumb = self.thumb();
        let travel = self.height - (thumb.end - thumb.start);
        if !self.scrolls() || travel <= 0.0 {
            return 0;
        }
        let fraction = ((thumb_top - self.top) / travel).clamp(0.0, 1.0);
        (fraction * (self.rows - self.shown) as f32).round() as usize
    }
}

/// Places the lanes one above the other in `height`. `first` holds each lane's requested
/// first row; it is limited so the last row is never followed by empty space.
pub fn lay_out(height: f32, lanes: &[LaneRows], first: &[usize]) -> Vec<LaneBox> {
    let height = height.max(0.0);
    let total: usize = lanes.iter().map(|lane| lane.rows).sum();
    let mut shown: Vec<usize> = lanes.iter().map(|lane| lane.rows).collect();
    let stacked = lanes
        .iter()
        .filter(|lane| lane.stacked && lane.rows > 0)
        .count();
    if stacked > 0 && total as f32 * MIN_ROW_HEIGHT > height {
        let fixed: usize = lanes
            .iter()
            .filter(|lane| !lane.stacked)
            .map(|lane| lane.rows)
            .sum();
        let fitting = (height / MIN_ROW_HEIGHT).floor() as usize;
        // Every stacked lane keeps at least one row, even when that squeezes all rows.
        let mut budget = fitting.saturating_sub(fixed).max(stacked);
        for (lane, count) in lanes.iter().zip(&mut shown) {
            if lane.stacked && lane.rows > 0 {
                *count = 1;
                budget -= 1;
            }
        }
        // Hand out the rest one row at a time to the lane showing the smallest part of itself.
        while budget > 0 {
            let next = lanes
                .iter()
                .zip(&shown)
                .enumerate()
                .filter(|(_, (lane, count))| lane.stacked && **count < lane.rows)
                .min_by(|(_, (a, a_count)), (_, (b, b_count))| {
                    let a = **a_count as f32 / a.rows as f32;
                    let b = **b_count as f32 / b.rows as f32;
                    a.total_cmp(&b)
                })
                .map(|(index, _)| index);
            let Some(index) = next else { break };
            shown[index] += 1;
            budget -= 1;
        }
    }
    let on_screen: usize = shown.iter().sum();
    let row_height = if on_screen == 0 {
        0.0
    } else {
        height / on_screen as f32
    };
    let mut top = 0.0;
    lanes
        .iter()
        .zip(shown)
        .enumerate()
        .map(|(index, (lane, shown))| {
            let first = first
                .get(index)
                .copied()
                .unwrap_or(0)
                .min(lane.rows - shown);
            let lane_box = LaneBox {
                top,
                height: shown as f32 * row_height,
                rows: lane.rows,
                first,
                shown,
                row_height,
            };
            top += lane_box.height;
            lane_box
        })
        .collect()
}

/// Places the lanes one above the other in `height` as one column of rows that scrolls as a
/// whole, for a plot with more lanes than per-lane scrolling suits. When every row has at
/// least `MIN_ROW_HEIGHT` this is [`lay_out`] with no lane scrolled. Otherwise as many whole
/// rows as fit are on screen, each at least `MIN_ROW_HEIGHT` tall (or the one row there is
/// room for, squeezed), from row `first` of the column; `first` is limited so the last row is
/// never followed by empty space. Each lane's box holds the part of the lane on screen, so a
/// lane can start part way down its rows; a lane above or below the rows on screen shows
/// none. The second value is the column itself, as if it were one lane of every row: its
/// `first`, `shown` and `thumb` drive the plot's scroll bar.
pub fn lay_out_column(height: f32, lanes: &[LaneRows], first: usize) -> (Vec<LaneBox>, LaneBox) {
    let height = height.max(0.0);
    let total: usize = lanes.iter().map(|lane| lane.rows).sum();
    let fitting = (height / MIN_ROW_HEIGHT).floor() as usize;
    if total <= fitting {
        let boxes = lay_out(height, lanes, &[]);
        let row_height = boxes.first().map_or(0.0, |lane| lane.row_height);
        let column = LaneBox {
            top: 0.0,
            height,
            rows: total,
            first: 0,
            shown: total,
            row_height,
        };
        return (boxes, column);
    }
    let shown = fitting.max(1).min(total);
    let row_height = height / shown as f32;
    let first = first.min(total - shown);
    let end = first + shown;
    let mut start = 0;
    let boxes = lanes
        .iter()
        .map(|lane| {
            let lane_end = start + lane.rows;
            let (from, to) = (start.max(first), lane_end.min(end));
            let lane_box = if from < to {
                LaneBox {
                    top: (from - first) as f32 * row_height,
                    height: (to - from) as f32 * row_height,
                    rows: lane.rows,
                    first: from - start,
                    shown: to - from,
                    row_height,
                }
            } else {
                // Scrolled out: above the rows on screen, every row is past; below, none is.
                let above = lane_end <= first;
                LaneBox {
                    top: if above { 0.0 } else { height },
                    height: 0.0,
                    rows: lane.rows,
                    first: if above { lane.rows } else { 0 },
                    shown: 0,
                    row_height,
                }
            };
            start = lane_end;
            lane_box
        })
        .collect();
    let column = LaneBox {
        top: 0.0,
        height,
        rows: total,
        first,
        shown,
        row_height,
    };
    (boxes, column)
}

/// The lane (index into `boxes`) and row under a height measured like the boxes' tops.
pub fn row_at(boxes: &[LaneBox], y: f32) -> Option<(usize, usize)> {
    boxes.iter().enumerate().find_map(|(index, lane)| {
        let offset = y - lane.top;
        if lane.shown == 0 || offset < 0.0 || offset >= lane.height {
            return None;
        }
        let row = ((offset / lane.row_height) as usize).min(lane.shown - 1);
        Some((index, lane.first + row))
    })
}

/// The audio height a window grows to so that every row is comfortable, when a lane is
/// stacked. Mono and stereo layouts never ask for more room.
pub fn comfortable_height(lanes: &[LaneRows]) -> Option<f32> {
    lanes
        .iter()
        .any(|lane| lane.stacked)
        .then(|| lanes.iter().map(|lane| lane.rows).sum::<usize>() as f32 * COMFORTABLE_ROW_HEIGHT)
}

/// A channel name short enough for the label column. Long names keep a trailing number
/// ("Object 12" shows "12") or their first four characters; the tooltip has the full name.
pub fn short_label(label: &str) -> String {
    let label = label.trim();
    if label.chars().count() <= 4 {
        return label.to_owned();
    }
    let digits: String = label
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if (1..=4).contains(&digits.len()) {
        digits
    } else {
        label.chars().take(4).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn narrow(rows: usize) -> LaneRows {
        LaneRows {
            rows,
            stacked: false,
        }
    }

    fn wide(rows: usize) -> LaneRows {
        LaneRows {
            rows,
            stacked: true,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn mono_and_stereo_lanes_split_the_height_as_before_however_small() {
        for height in [30.0, 43.0, 59.0, 200.0] {
            let one = lay_out(height, &[narrow(1)], &[]);
            assert_eq!(one.len(), 1);
            assert_eq!((one[0].rows, one[0].shown), (1, 1));
            assert!(close(one[0].row_height, height));
            // Left / Right, and A above B, are two rows of half the height each.
            let two = lay_out(height, &[narrow(2)], &[]);
            assert!(close(two[0].row_height, height / 2.0));
            let dual = lay_out(height, &[narrow(1), narrow(1)], &[]);
            assert!(close(dual[1].top, height / 2.0) && close(dual[1].height, height / 2.0));
        }
    }

    #[test]
    fn wide_lanes_show_every_row_when_each_gets_the_minimum() {
        for channels in [12usize, 16, 40] {
            let height = channels as f32 * MIN_ROW_HEIGHT;
            let lane = lay_out(height, &[wide(channels)], &[3])[0];
            assert_eq!((lane.rows, lane.shown, lane.first), (channels, channels, 0));
            assert!(!lane.scrolls());
            assert!(close(lane.row_height, MIN_ROW_HEIGHT));
            let roomy = lay_out(height * 2.0, &[wide(channels)], &[])[0];
            assert!(close(roomy.row_height, MIN_ROW_HEIGHT * 2.0));
        }
    }

    #[test]
    fn rows_that_do_not_fit_scroll_and_keep_the_minimum_height() {
        // (channels, audio height, rows on screen)
        for (channels, height, shown) in [
            (12, 47.0, 3), // the smallest full window
            (12, 48.0, 3), // the smallest waveform-only window
            (12, 100.0, 7),
            (12, 167.0, 11),
            (16, 100.0, 7),
            (16, 223.0, 15),
            (40, 100.0, 7),
            (40, 400.0, 28),
            (40, 559.0, 39),
        ] {
            let lane = lay_out(height, &[wide(channels)], &[])[0];
            assert_eq!(lane.shown, shown, "{channels} channels in {height}");
            assert!(lane.scrolls());
            assert!(lane.row_height >= MIN_ROW_HEIGHT && lane.row_height < MIN_ROW_HEIGHT * 2.0);
            assert!(close(lane.height, height));
        }
    }

    #[test]
    fn lane_b_keeps_its_row_below_a_wide_lane_a() {
        for height in [47.0, 59.0, 120.0, 300.0] {
            let [a, b] = lay_out(height, &[wide(12), narrow(1)], &[])[..] else {
                panic!()
            };
            assert_eq!(b.shown, 1, "lane B is never scrolled away");
            assert!(
                close(b.row_height, a.row_height),
                "all rows on screen match"
            );
            assert!(b.row_height >= MIN_ROW_HEIGHT);
            assert!(close(b.top, a.height) && close(b.top + b.height, height));
        }
        // Two wide lanes share the room in proportion to their size.
        let [a, b] = lay_out(14.0 * 10.0, &[wide(12), wide(4)], &[])[..] else {
            panic!()
        };
        assert_eq!((a.shown, b.shown), (7, 3));
        let [a, b] = lay_out(30.0, &[wide(40), wide(2)], &[])[..] else {
            panic!()
        };
        assert_eq!(
            (a.shown, b.shown),
            (1, 1),
            "each stacked lane keeps one row"
        );
    }

    #[test]
    fn a_window_too_small_for_one_row_each_squeezes_rows_rather_than_hide_a_lane() {
        let boxes = lay_out(20.0, &[wide(12), narrow(1)], &[]);
        assert_eq!(
            boxes.iter().map(|lane| lane.shown).collect::<Vec<_>>(),
            [1, 1]
        );
        assert!(close(boxes[0].row_height, 10.0));
        let empty = lay_out(0.0, &[wide(12)], &[]);
        assert_eq!(empty[0].shown, 1);
        assert!(close(empty[0].height, 0.0));
    }

    #[test]
    fn scrolling_shows_the_requested_rows_and_never_runs_past_the_last() {
        let lane = lay_out(100.0, &[wide(12)], &[4])[0];
        assert_eq!(lane.visible(), 4..11);
        assert_eq!(lane.row_top(3), None);
        assert!(close(lane.row_top(4).unwrap(), 0.0));
        assert!(close(lane.row_top(10).unwrap(), 6.0 * lane.row_height));
        assert_eq!(lane.row_top(11), None);
        let end = lay_out(100.0, &[wide(12)], &[50])[0];
        assert_eq!(end.visible(), 5..12);
        let fits = lay_out(400.0, &[wide(12)], &[50])[0];
        assert_eq!(fits.first, 0);
    }

    #[test]
    fn a_point_finds_its_lane_and_row() {
        let boxes = lay_out(150.0, &[wide(12), narrow(1)], &[2, 0]);
        let a = boxes[0];
        assert_eq!(a.shown, 9);
        let row = a.row_height;
        assert_eq!(row_at(&boxes, 0.0), Some((0, 2)));
        assert_eq!(row_at(&boxes, row * 3.5), Some((0, 5)));
        assert_eq!(row_at(&boxes, a.height - 0.01), Some((0, 10)));
        assert_eq!(row_at(&boxes, a.height + 0.01), Some((1, 0)));
        assert_eq!(row_at(&boxes, 149.99), Some((1, 0)));
        assert_eq!(row_at(&boxes, -1.0), None);
        assert_eq!(row_at(&boxes, 150.0), None);
    }

    #[test]
    fn the_scroll_thumb_follows_the_first_row_both_ways() {
        let lane = lay_out(100.0, &[wide(40)], &[0])[0];
        assert_eq!(lane.shown, 7);
        let top = lane.thumb();
        assert!(close(top.start, 0.0) && close(top.end - top.start, 100.0 * 7.0 / 40.0));
        let last = lay_out(100.0, &[wide(40)], &[33])[0];
        assert!(close(last.thumb().end, 100.0));
        for first in [0, 1, 10, 20, 33] {
            let lane = lay_out(100.0, &[wide(40)], &[first])[0];
            assert_eq!(lane.first_for_thumb(lane.thumb().start), first);
        }
        assert_eq!(lane.first_for_thumb(-50.0), 0);
        assert_eq!(lane.first_for_thumb(500.0), 33);
        let many = lay_out(48.0, &[wide(64)], &[0])[0];
        assert!(many.thumb().end - many.thumb().start >= MIN_THUMB);
    }

    #[test]
    fn a_column_that_fits_is_laid_out_as_before() {
        let lanes = [narrow(1), wide(8), narrow(1)];
        for height in [140.0, 200.0, 560.0] {
            let (boxes, column) = lay_out_column(height, &lanes, 5);
            assert_eq!(boxes, lay_out(height, &lanes, &[]));
            assert_eq!((column.rows, column.first, column.shown), (10, 0, 10));
            assert!(!column.scrolls());
        }
    }

    #[test]
    fn a_column_too_tall_scrolls_whole_rows_at_least_the_minimum_high() {
        // Sixty-four mono lanes in the tallest grown window's plot.
        let lanes = vec![narrow(1); 64];
        let (boxes, column) = lay_out_column(500.0, &lanes, 0);
        assert_eq!((column.rows, column.first, column.shown), (64, 0, 35));
        assert!(column.scrolls());
        assert!(column.row_height >= MIN_ROW_HEIGHT && column.row_height < MIN_ROW_HEIGHT * 2.0);
        assert_eq!(boxes.iter().filter(|lane| lane.shown == 1).count(), 35);
        assert!(close(boxes[34].top + boxes[34].height, 500.0));
        // Lanes below the rows on screen show none and sit at the bottom.
        assert_eq!((boxes[35].shown, boxes[35].first), (0, 0));
        assert!(close(boxes[63].top, 500.0) && close(boxes[63].height, 0.0));
        // Scrolled to the middle and past the end, which stops at the last row.
        let (boxes, column) = lay_out_column(500.0, &lanes, 20);
        assert_eq!(column.visible(), 20..55);
        assert_eq!((boxes[19].shown, boxes[19].first), (0, 1));
        assert!(close(boxes[20].top, 0.0));
        let (boxes, column) = lay_out_column(500.0, &lanes, 99);
        assert_eq!(column.visible(), 29..64);
        assert!(close(boxes[63].top + boxes[63].height, 500.0));
        // Every row on screen is found where it is drawn.
        for y in [0.0, 13.0, 250.0, 499.9] {
            let (lane, row) = row_at(&boxes, y).unwrap();
            assert_eq!(row, 0);
            assert_eq!(lane, 29 + (y / column.row_height) as usize);
        }
    }

    #[test]
    fn a_column_shows_part_of_a_wide_lane_and_its_rows_in_order() {
        // A, a 7.1.4 lane with its pair (14 rows), then C: 16 rows in 100 points.
        let lanes = [narrow(1), wide(14), narrow(1)];
        let (boxes, column) = lay_out_column(100.0, &lanes, 4);
        assert_eq!(column.shown, 7);
        assert_eq!(column.visible(), 4..11);
        assert_eq!(boxes[0].shown, 0);
        let b = boxes[1];
        assert_eq!((b.first, b.shown), (3, 7));
        assert_eq!(b.visible(), 3..10);
        assert!(close(b.top, 0.0) && close(b.height, 100.0));
        assert_eq!(boxes[2].shown, 0);
        // The bottom: the last rows of B, then C.
        let (boxes, column) = lay_out_column(100.0, &lanes, 50);
        assert_eq!(column.visible(), 9..16);
        assert_eq!(boxes[1].visible(), 8..14);
        assert_eq!(boxes[2].visible(), 0..1);
        assert!(close(boxes[2].top, 6.0 * column.row_height));
        // The column's thumb follows its first row both ways, as a lane's does.
        for first in [0, 3, 9] {
            let (_, column) = lay_out_column(100.0, &lanes, first);
            assert_eq!(column.first_for_thumb(column.thumb().start), first);
        }
    }

    #[test]
    fn a_column_too_small_for_one_row_squeezes_it() {
        let (boxes, column) = lay_out_column(10.0, &[narrow(1), narrow(1), narrow(1)], 1);
        assert_eq!((column.first, column.shown), (1, 1));
        assert!(close(column.row_height, 10.0));
        assert_eq!(boxes[1].shown, 1);
        let (boxes, column) = lay_out_column(0.0, &[narrow(1), narrow(1), narrow(1)], 0);
        assert_eq!(column.shown, 1);
        assert!(close(boxes[0].height, 0.0));
        let (boxes, column) = lay_out_column(100.0, &[], 3);
        assert!(boxes.is_empty() && column.rows == 0);
    }

    #[test]
    fn only_stacked_layouts_ask_for_a_taller_window() {
        assert_eq!(comfortable_height(&[narrow(2)]), None);
        assert_eq!(comfortable_height(&[narrow(1), narrow(1)]), None);
        assert_eq!(comfortable_height(&[wide(12)]), Some(240.0));
        assert_eq!(comfortable_height(&[wide(12), narrow(1)]), Some(260.0));
    }

    #[test]
    fn labels_fit_the_column_and_keep_what_tells_them_apart() {
        assert_eq!(short_label("LFE"), "LFE");
        assert_eq!(short_label("Ltf"), "Ltf");
        assert_eq!(short_label("Channel 12"), "12");
        assert_eq!(short_label("Object 3"), "3");
        assert_eq!(short_label("Center"), "Cent");
        assert_eq!(short_label(" L "), "L");
    }
}
