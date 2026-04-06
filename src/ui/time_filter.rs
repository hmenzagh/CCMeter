use std::collections::HashMap;

use chrono::{Local, NaiveDate};

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
            TimeFilter::Today | TimeFilter::Hour1 | TimeFilter::Hour12 => Self {
                start: today,
                end: today,
                all: false,
            },
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
