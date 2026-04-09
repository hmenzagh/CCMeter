use std::collections::HashMap;

use chrono::{Local, NaiveDate, Timelike};

use crate::data::tokens;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum TimeFilter {
    Hour1,
    Hour12,
    Today,
    LastWeek,
    LastMonth,
    All,
}

impl TimeFilter {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            TimeFilter::Hour1 => "1h",
            TimeFilter::Hour12 => "12h",
            TimeFilter::Today => "Today",
            TimeFilter::LastWeek => "Last week",
            TimeFilter::LastMonth => "Last month",
            TimeFilter::All => "All",
        }
    }

    pub(crate) fn index(&self) -> usize {
        match self {
            TimeFilter::All => 0,
            TimeFilter::LastMonth => 1,
            TimeFilter::LastWeek => 2,
            TimeFilter::Today => 3,
            TimeFilter::Hour12 => 4,
            TimeFilter::Hour1 => 5,
        }
    }

    pub(crate) fn next(&self) -> TimeFilter {
        match self {
            TimeFilter::All => TimeFilter::LastMonth,
            TimeFilter::LastMonth => TimeFilter::LastWeek,
            TimeFilter::LastWeek => TimeFilter::Today,
            TimeFilter::Today => TimeFilter::Hour12,
            TimeFilter::Hour12 => TimeFilter::Hour1,
            TimeFilter::Hour1 => TimeFilter::All,
        }
    }

    pub(crate) fn is_intraday(&self) -> bool {
        matches!(
            self,
            TimeFilter::Hour1 | TimeFilter::Hour12 | TimeFilter::Today
        )
    }

    /// Returns `(start_date, start_minute)` for sub-day filters that need
    /// minute-level filtering. Correctly crosses midnight when the window
    /// extends into the previous day. Returns `None` for Today and non-intraday
    /// filters (they use full-day data).
    pub(crate) fn subday_start(&self) -> Option<(NaiveDate, u16)> {
        let now = chrono::Local::now();
        let today = now.date_naive();
        let current_minute = now.hour() as u16 * 60 + now.minute() as u16;
        let offset = match self {
            TimeFilter::Hour1 => 60u16,
            TimeFilter::Hour12 => 720u16,
            _ => return None,
        };
        if current_minute >= offset {
            Some((today, current_minute - offset))
        } else {
            let yesterday = today - chrono::Duration::days(1);
            Some((yesterday, 1440 - (offset - current_minute)))
        }
    }

    pub(crate) const ALL: &'static [TimeFilter] = &[
        TimeFilter::All,
        TimeFilter::LastMonth,
        TimeFilter::LastWeek,
        TimeFilter::Today,
        TimeFilter::Hour12,
        TimeFilter::Hour1,
    ];
}

pub(crate) fn filter_daily(daily: &tokens::DailyTokens, filter: TimeFilter) -> tokens::DailyTokens {
    if filter == TimeFilter::All {
        return daily.clone();
    }

    let today = Local::now().date_naive();
    let pred = DatePredicate::new(filter, today);

    tokens::DailyTokens {
        input: filter_map(&daily.input, &pred),
        output: filter_map(&daily.output, &pred),
        lines_suggested: filter_map(&daily.lines_suggested, &pred),
        lines_accepted: filter_map(&daily.lines_accepted, &pred),
        lines_added: filter_map(&daily.lines_added, &pred),
        lines_deleted: filter_map(&daily.lines_deleted, &pred),
        cost: filter_map(&daily.cost, &pred),
    }
}

pub(crate) fn date_in_filter(date: NaiveDate, filter: TimeFilter, today: NaiveDate) -> bool {
    DatePredicate::new(filter, today).matches(date)
}

/// Pre-computed date range for fast filtering without re-deriving bounds per call.
struct DatePredicate {
    start: NaiveDate,
    end: NaiveDate,
    all: bool,
}

impl DatePredicate {
    fn new(filter: TimeFilter, today: NaiveDate) -> Self {
        match filter {
            TimeFilter::All => Self {
                start: NaiveDate::MIN,
                end: NaiveDate::MAX,
                all: true,
            },
            TimeFilter::Today => Self {
                start: today,
                end: today,
                all: false,
            },
            TimeFilter::Hour1 | TimeFilter::Hour12 => {
                // Include yesterday if the window crosses midnight.
                let start = filter.subday_start().map(|(d, _)| d).unwrap_or(today);
                Self {
                    start,
                    end: today,
                    all: false,
                }
            }
            TimeFilter::LastWeek => Self {
                start: today - chrono::Duration::days(6),
                end: today,
                all: false,
            },
            TimeFilter::LastMonth => Self {
                start: today - chrono::Duration::days(29),
                end: today,
                all: false,
            },
        }
    }

    fn matches(&self, date: NaiveDate) -> bool {
        self.all || (date >= self.start && date <= self.end)
    }
}

fn filter_map<V: Copy>(
    daily: &HashMap<NaiveDate, V>,
    pred: &DatePredicate,
) -> HashMap<NaiveDate, V> {
    let mut result = HashMap::with_capacity(daily.len() / 2);
    for (&date, &val) in daily {
        if pred.matches(date) {
            result.insert(date, val);
        }
    }
    result
}
