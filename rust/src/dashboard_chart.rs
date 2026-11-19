//! Timestamped activity from the currently loaded, bounded recording windows.
//! Rendering performs no recording reads and uses one canvas for the entire plot.
use crate::style::Palette;
use agentwatcher::activity::AgentActivity;
use gpui::{
    App, Bounds, Div, FontWeight, Pixels, Window, canvas, div, fill, point, prelude::*, px, rgb,
    rgba, size,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

const PLOT_HEIGHT: f32 = 140.;
const KINDS: [&str; 6] = ["User", "Agent", "Tool", "Completed", "Error", "Other"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartBin {
    /// Inclusive Unix minute.
    pub start_minute: u64,
    /// Exclusive Unix minute.
    pub end_minute: u64,
    pub counts: [u32; 6],
}

impl ChartBin {
    pub fn total(&self) -> u64 {
        self.counts.iter().map(|count| u64::from(*count)).sum()
    }
}

/// Absolute, nonoverlapping time ranges, including the current minute. The
/// hour view has 60 one-minute bins. The day view uses fixed fifteen-minute
/// boundaries, clipping the first and last bins to the exact 1440-minute window
/// (96 or 97 bins). Historical selections therefore survive minute rollover.
/// Pre-epoch time is clipped rather than wrapped to a future date.
pub fn bins(activities: &[AgentActivity], hours: u64, now_seconds: u64) -> Vec<ChartBin> {
    let end = now_seconds / 60 + 1;
    let start = end.saturating_sub(if hours == 1 { 60 } else { 1440 });
    let mut result = if hours == 1 {
        (0..60)
            .map(|index| ChartBin {
                start_minute: end.saturating_sub(60 - index),
                end_minute: end.saturating_sub(59 - index),
                counts: [0; 6],
            })
            .collect::<Vec<_>>()
    } else {
        let mut result = Vec::with_capacity(97);
        let mut cursor = start;
        while cursor < end {
            let next = ((cursor / 15 + 1) * 15).min(end);
            result.push(ChartBin {
                start_minute: cursor,
                end_minute: next,
                counts: [0; 6],
            });
            cursor = next;
        }
        result
    };
    for activity in activities {
        for bucket in &activity.timeline {
            if bucket.minute < start || bucket.minute >= end {
                continue;
            }
            let index = result.partition_point(|bin| bin.end_minute <= bucket.minute);
            for (total, count) in result[index].counts.iter_mut().zip(bucket.counts) {
                *total = total.saturating_add(count);
            }
        }
    }
    result
}

fn label(value: impl Into<String>, color: u32) -> Div {
    div()
        .min_w_0()
        .text_size(px(11.))
        .text_color(rgb(color))
        .child(value.into())
}

fn local_time(minute: u64, date: bool) -> String {
    let Some(seconds) = minute
        .checked_mul(60)
        .and_then(|value| libc::time_t::try_from(value).ok())
    else {
        return format!("minute {minute}");
    };
    // localtime_r writes to caller-owned memory, unlike localtime's shared buffer.
    unsafe {
        let mut parts = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&seconds, &mut parts).is_null() {
            return format!("minute {minute}");
        }
        let mut buffer = [0_u8; 64];
        let format = if date { c"%b %d, %H:%M %Z" } else { c"%H:%M" };
        let length = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            format.as_ptr(),
            &parts,
        );
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}

/// Four readable count intervals, with a scale large enough for the tallest bin.
fn axis_max(maximum: u64) -> u64 {
    let desired = maximum.max(1).div_ceil(4);
    let magnitude = 10_u64.pow(desired.ilog10());
    let step = [1_u64, 2, 5, 10]
        .into_iter()
        .map(|factor| factor.saturating_mul(magnitude))
        .find(|step| *step >= desired)
        .unwrap_or(desired);
    step.saturating_mul(4)
}

pub fn chart(
    activities: &[AgentActivity],
    hours: u64,
    selected_minute: Option<u64>,
    p: Palette,
    on_select: Rc<dyn Fn(u64, &mut Window, &mut App)>,
) -> Div {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let data = Rc::new(bins(activities, hours, now));
    let first = data.first().expect("chart has bins").start_minute;
    let end = data.last().expect("chart has bins").end_minute;
    let span = (end - first) as f32;
    let selected = selected_minute.and_then(|minute| {
        data.iter()
            .position(|bin| bin.start_minute <= minute && minute < bin.end_minute)
    });
    let total = data.iter().map(ChartBin::total).sum::<u64>();
    let maximum = axis_max(data.iter().map(ChartBin::total).max().unwrap_or(0));
    let loaded = activities.iter().filter(|activity| activity.loaded).count();
    let colors = [p.accent, p.syntax_keyword, p.muted, p.green, p.red, p.faint];
    let plot_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
    let prepaint_bounds = plot_bounds.clone();
    let painted_data = data.clone();
    let click_data = data.clone();

    let drawing = canvas(
        move |bounds, _, _| prepaint_bounds.set(Some(bounds)),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            if width <= 0. || height <= 0. {
                return;
            }
            let bin_geometry = |bin: &ChartBin| {
                let x = width * (bin.start_minute - first) as f32 / span;
                let bin_width = width * (bin.end_minute - bin.start_minute) as f32 / span;
                (x, bin_width)
            };
            if let Some(index) = selected {
                let (x, bin_width) = bin_geometry(&painted_data[index]);
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left() + px(x), bounds.top()),
                        size(px(bin_width), bounds.size.height),
                    ),
                    rgba((p.accent << 8) | 0x18),
                ));
            }
            for line in 0..=4 {
                let y = bounds.top() + px((height - 1.) * line as f32 / 4.);
                window.paint_quad(fill(
                    Bounds::new(point(bounds.left(), y), size(bounds.size.width, px(1.))),
                    rgb(p.border),
                ));
            }
            for bin in painted_data.iter() {
                let (offset, bin_width) = bin_geometry(bin);
                let bar_width = (bin_width - 2.).max(bin_width * 0.65).min(bin_width);
                let x = bounds.left() + px(offset + (bin_width - bar_width) / 2.);
                let mut bottom = bounds.bottom() - px(1.);
                for (count, color) in bin.counts.iter().zip(colors) {
                    if *count == 0 {
                        continue;
                    }
                    let segment = (height - 1.) * *count as f32 / maximum as f32;
                    bottom -= px(segment);
                    window.paint_quad(fill(
                        Bounds::new(point(x, bottom), size(px(bar_width), px(segment))),
                        rgb(color),
                    ));
                }
            }
            if let Some(index) = selected {
                let (offset, bin_width) = bin_geometry(&painted_data[index]);
                let x = bounds.left() + px(offset + bin_width / 2.);
                window.paint_quad(fill(
                    Bounds::new(point(x, bounds.top()), size(px(1.), bounds.size.height)),
                    rgb(p.accent),
                ));
                window.paint_quad(fill(
                    Bounds::new(point(x - px(2.), bounds.top()), size(px(5.), px(3.))),
                    rgb(p.accent),
                ));
            }
        },
    )
    .w_full()
    .h(px(PLOT_HEIGHT));

    let mut plot = div()
        .id("dashboard-activity-plot")
        .relative()
        .w_full()
        .min_w_0()
        .h(px(PLOT_HEIGHT))
        .cursor_pointer()
        .on_click(move |event, window, cx| {
            if event.is_right_click() {
                return;
            }
            let (Some(bounds), Some(position)) = (plot_bounds.get(), event.mouse_position()) else {
                return;
            };
            if !bounds.contains(&position) || bounds.size.width <= px(0.) {
                return;
            }
            let fraction = f32::from(position.x - bounds.left()) / f32::from(bounds.size.width);
            let minute = (first + (fraction * span) as u64).min(end - 1);
            let index = click_data.partition_point(|bin| bin.end_minute <= minute);
            let bin = &click_data[index];
            if bin.start_minute < bin.end_minute {
                on_select(bin.start_minute, window, cx);
            }
        })
        .child(drawing);
    if total == 0 {
        plot = plot.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(5.))
                .child(
                    label("No timestamped activity in this window", p.text)
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(label(
                    "Events appear as recording windows are loaded.",
                    p.faint,
                )),
        );
    }

    let mut y_axis = div()
        .relative()
        .w(px(44.))
        .h(px(PLOT_HEIGHT))
        .flex_shrink_0();
    for tick in (0..=4).rev() {
        y_axis = y_axis.child(
            label(number(maximum * tick / 4), p.faint)
                .absolute()
                .top(px((PLOT_HEIGHT - 1.) * (4 - tick) as f32 / 4. - 7.))
                .w(px(36.))
                .h(px(14.))
                .line_height(px(14.))
                .text_right()
                .line_clamp(1),
        );
    }
    let mut x_axis = div().flex().justify_between().w_full().min_w_0().mt(px(7.));
    for tick in 0..=4 {
        let tick_label = label(local_time(first + (end - first) * tick / 4, false), p.faint)
            .w(px(40.))
            .flex_shrink_0()
            .line_clamp(1);
        x_axis = x_axis.child(if tick == 0 {
            tick_label
        } else if tick == 4 {
            tick_label.text_right()
        } else {
            tick_label.text_center()
        });
    }

    let mut legend = div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(13.));
    for (kind, color) in KINDS.into_iter().zip(colors) {
        legend = legend.child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(5.))
                .child(div().size(px(6.)).rounded(px(1.)).bg(rgb(color)))
                .child(label(kind, p.muted).whitespace_nowrap()),
        );
    }
    let caption = if let Some(index) = selected {
        let bin = &data[index];
        let details = KINDS
            .iter()
            .zip(bin.counts)
            .map(|(kind, count)| format!("{} {}", number(u64::from(count)), kind.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                label(
                    format!(
                        "{} — {} · {} events",
                        local_time(bin.start_minute, true),
                        local_time(bin.end_minute, true),
                        number(bin.total()),
                    ),
                    p.text,
                )
                .w_full()
                .line_clamp(2),
            )
            .child(label(details, p.muted).w_full().line_clamp(2))
    } else {
        label("Select a time slice to inspect its events.", p.faint)
            .w_full()
            .line_clamp(2)
    };

    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .min_w_0()
                .gap(px(12.))
                .child(
                    label(format!("{} timestamped events", number(total)), p.text)
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .line_clamp(1),
                )
                .child(
                    label(if hours == 1 { "Last 1 hour" } else { "Last 24 hours" }, p.muted)
                        .w(px(90.))
                        .flex_shrink_0()
                        .text_right()
                        .line_clamp(1),
                ),
        )
        .child(
            div()
                .w_full()
                .flex()
                .min_w_0()
                .child(y_axis)
                .child(div().flex_1().min_w_0().child(plot).child(x_axis)),
        )
        .child(legend)
        .child(caption)
        .child(
            label(
                format!("All monitored workspaces · {loaded} loaded recording windows · counts may omit older or unread events"),
                p.faint,
            )
            .w_full()
            .line_clamp(2),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentwatcher::activity::ActivityBucket;

    fn activity(values: &[(u64, [u32; 6])]) -> AgentActivity {
        AgentActivity {
            loaded: true,
            timeline: values
                .iter()
                .map(|(minute, counts)| ActivityBucket {
                    minute: *minute,
                    counts: *counts,
                })
                .collect(),
            ..AgentActivity::default()
        }
    }

    #[test]
    fn hour_window_counts_both_edges_and_excludes_stale_and_future_minutes() {
        let now = 30_000_u64;
        let first = now - 59;
        let data = bins(
            &[activity(&[
                (first - 1, [90, 0, 0, 0, 0, 0]),
                (first, [2, 1, 0, 0, 0, 0]),
                (now, [0, 0, 3, 1, 1, 0]),
                (now + 1, [90, 0, 0, 0, 0, 0]),
            ])],
            1,
            now * 60 + 17,
        );
        assert_eq!(data.len(), 60);
        assert_eq!(
            (data[0].start_minute, data[59].end_minute),
            (first, now + 1)
        );
        assert!(
            data.windows(2)
                .all(|pair| pair[0].end_minute == pair[1].start_minute)
        );
        assert_eq!(data[0].counts, [2, 1, 0, 0, 0, 0]);
        assert_eq!(data[59].counts, [0, 0, 3, 1, 1, 0]);
        assert_eq!(data.iter().map(ChartBin::total).sum::<u64>(), 8);
    }

    #[test]
    fn day_window_aggregates_exact_edges_and_fixed_quarter_hour_boundaries() {
        let now = 40_000_u64;
        let first = now + 1 - 1440;
        let boundary = 38_565;
        let data = bins(
            &[
                activity(&[
                    (first - 1, [90, 0, 0, 0, 0, 0]),
                    (boundary - 1, [1, 2, 0, 0, 0, 0]),
                    (boundary, [0, 0, 3, 0, 0, 0]),
                ]),
                activity(&[
                    (boundary - 1, [4, 0, 0, 1, 0, 0]),
                    (now, [0, 0, 0, 0, 2, 1]),
                    (now + 1, [90, 0, 0, 0, 0, 0]),
                ]),
                AgentActivity::default(),
            ],
            24,
            now * 60,
        );
        assert_eq!(data.len(), 97);
        assert_eq!(
            (data[0].start_minute, data[0].end_minute),
            (first, boundary)
        );
        assert_eq!(data.last().unwrap().end_minute, now + 1);
        assert!(
            data[1..data.len() - 1].iter().all(|bin| {
                bin.start_minute % 15 == 0 && bin.end_minute - bin.start_minute == 15
            })
        );
        assert!(
            data.windows(2)
                .all(|pair| pair[0].end_minute == pair[1].start_minute)
        );
        assert_eq!(data[0].counts, [5, 2, 0, 1, 0, 0]);
        assert_eq!(data[1].counts, [0, 0, 3, 0, 0, 0]);
        assert_eq!(data.last().unwrap().counts, [0, 0, 0, 0, 2, 1]);
        assert_eq!(data.iter().map(ChartBin::total).sum::<u64>(), 14);
        assert_eq!(
            data.iter()
                .map(|bin| bin.end_minute - bin.start_minute)
                .sum::<u64>(),
            1440
        );

        let aligned = bins(&[], 24, 39_989 * 60);
        assert_eq!(aligned.len(), 96);
        assert!(
            aligned
                .iter()
                .all(|bin| bin.end_minute - bin.start_minute == 15)
        );
    }

    #[test]
    fn selected_historical_slice_keeps_its_range_and_counts_after_minute_rollover() {
        let recordings = [activity(&[
            (39_945, [2, 0, 0, 0, 0, 0]),
            (39_959, [0, 3, 0, 0, 0, 0]),
            (39_960, [0, 0, 7, 0, 0, 0]),
            (40_002, [90, 0, 0, 0, 0, 0]),
        ])];
        let before = bins(&recordings, 24, 40_000 * 60);
        let selected = before
            .iter()
            .find(|bin| bin.start_minute <= 39_950 && 39_950 < bin.end_minute)
            .unwrap();
        let selected_minute = selected.start_minute;
        let after = bins(&recordings, 24, 40_001 * 60);
        let retained = after
            .iter()
            .find(|bin| bin.start_minute <= selected_minute && selected_minute < bin.end_minute)
            .unwrap();
        assert_eq!(
            (selected.start_minute, selected.end_minute),
            (39_945, 39_960)
        );
        assert_eq!(selected, retained);
        assert_eq!(retained.total(), 5);
        assert_eq!(after.iter().map(ChartBin::total).sum::<u64>(), 12);
    }

    #[test]
    fn empty_recordings_and_unsupported_ranges_have_an_honest_empty_day() {
        let empty = [AgentActivity::default()];
        let data = bins(&empty, 7, 60 * 100_000);
        assert_eq!(data, bins(&empty, 24, 60 * 100_000));
        assert!(data.iter().all(|bin| bin.total() == 0));
    }

    #[test]
    fn epoch_window_never_wraps_or_duplicates_events() {
        let data = bins(
            &[activity(&[
                (0, [1, 0, 0, 0, 0, 0]),
                (1, [20, 0, 0, 0, 0, 0]),
            ])],
            1,
            0,
        );
        assert!(data.iter().all(|bin| bin.start_minute <= bin.end_minute));
        assert!(
            data.windows(2)
                .all(|pair| pair[0].end_minute == pair[1].start_minute)
        );
        assert_eq!(data.iter().map(ChartBin::total).sum::<u64>(), 1);
        assert_eq!(data.last().unwrap().counts[0], 1);
    }
}
