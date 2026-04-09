use std::collections::HashMap;

use chrono::{Local, NaiveDate};

#[derive(Default, Clone)]
pub struct DailyTokens {
    pub input: HashMap<NaiveDate, u64>,
    pub output: HashMap<NaiveDate, u64>,
    pub lines_suggested: HashMap<NaiveDate, u64>,
    pub lines_accepted: HashMap<NaiveDate, u64>,
    pub lines_added: HashMap<NaiveDate, u64>,
    pub lines_deleted: HashMap<NaiveDate, u64>,
    pub cost: HashMap<NaiveDate, f64>,
}

impl DailyTokens {
    /// Per-day acceptance rate (0–100) for the heatmap grid. Only days with suggestions.
    pub fn accept_rate_map(&self) -> HashMap<NaiveDate, u64> {
        let mut out = HashMap::new();
        for (&date, &sug) in &self.lines_suggested {
            if sug > 0 {
                let acc = self.lines_accepted.get(&date).copied().unwrap_or(0);
                let rate = ((acc as f64 / sug as f64) * 100.0).min(100.0) as u64;
                out.insert(date, rate);
            }
        }
        out
    }

    /// Overall acceptance rate as a weighted percentage.
    pub fn overall_accept_rate(&self) -> f64 {
        let total_sug: u64 = self.lines_suggested.values().sum();
        if total_sug == 0 {
            return 0.0;
        }
        let total_acc: u64 = self.lines_accepted.values().sum();
        ((total_acc as f64 / total_sug as f64) * 100.0).min(100.0)
    }

    pub fn total_cost(&self) -> f64 {
        self.cost.values().sum()
    }

    pub fn current_streak(&self) -> u32 {
        let today = Local::now().date_naive();
        let mut streak = 0u32;
        let mut day = today;
        loop {
            let has_input = self.input.get(&day).copied().unwrap_or(0) > 0;
            let has_output = self.output.get(&day).copied().unwrap_or(0) > 0;
            if has_input || has_output {
                streak += 1;
                day -= chrono::Duration::days(1);
            } else {
                break;
            }
        }
        streak
    }

    pub fn active_and_total_days(&self) -> (usize, usize) {
        let all_dates: std::collections::HashSet<NaiveDate> = self
            .input
            .keys()
            .chain(self.output.keys())
            .copied()
            .collect();
        let active = all_dates.len();
        if active == 0 {
            return (0, 0);
        }
        let Some(&min) = all_dates.iter().min() else {
            return (0, 0);
        };
        let Some(&max) = all_dates.iter().max() else {
            return (0, 0);
        };
        let total = (max - min).num_days() as usize + 1;
        (active, total)
    }

    pub fn avg_tokens_per_active_day(&self) -> u64 {
        let (active, _) = self.active_and_total_days();
        if active == 0 {
            return 0;
        }
        let total: u64 = self.input.values().sum::<u64>() + self.output.values().sum::<u64>();
        total / active as u64
    }

    pub fn avg_efficiency(&self) -> f64 {
        let total_tokens: u64 =
            self.input.values().sum::<u64>() + self.output.values().sum::<u64>();
        let total_lines: u64 =
            self.lines_added.values().sum::<u64>() + self.lines_deleted.values().sum::<u64>();
        if total_lines > 0 {
            total_tokens as f64 / total_lines as f64
        } else {
            0.0
        }
    }
}

#[derive(Default)]
pub struct MinuteTokens {
    pub input: HashMap<(NaiveDate, u16), u64>,
    pub output: HashMap<(NaiveDate, u16), u64>,
    pub lines_accepted: HashMap<(NaiveDate, u16), u64>,
    pub lines_suggested: HashMap<(NaiveDate, u16), u64>,
    pub lines_added: HashMap<(NaiveDate, u16), u64>,
    pub lines_deleted: HashMap<(NaiveDate, u16), u64>,
    pub cost: HashMap<(NaiveDate, u16), f64>,
}

impl MinuteTokens {
    /// Aggregate minute-level data into a `DailyTokens` keeping only entries
    /// whose minute-of-day is >= `min_minute` on the given `today` date.
    pub fn to_daily_filtered(&self, today: NaiveDate, min_minute: u16) -> DailyTokens {
        fn fold_u64(
            src: &HashMap<(NaiveDate, u16), u64>,
            dst: &mut HashMap<NaiveDate, u64>,
            today: NaiveDate,
            min_minute: u16,
        ) {
            for (&(date, minute), &val) in src {
                if date == today && minute >= min_minute {
                    *dst.entry(date).or_default() += val;
                }
            }
        }

        let mut dt = DailyTokens::default();
        fold_u64(&self.input, &mut dt.input, today, min_minute);
        fold_u64(&self.output, &mut dt.output, today, min_minute);
        fold_u64(
            &self.lines_suggested,
            &mut dt.lines_suggested,
            today,
            min_minute,
        );
        fold_u64(
            &self.lines_accepted,
            &mut dt.lines_accepted,
            today,
            min_minute,
        );
        fold_u64(&self.lines_added, &mut dt.lines_added, today, min_minute);
        fold_u64(
            &self.lines_deleted,
            &mut dt.lines_deleted,
            today,
            min_minute,
        );
        for (&(date, minute), &val) in &self.cost {
            if date == today && minute >= min_minute {
                *dt.cost.entry(date).or_default() += val;
            }
        }
        dt
    }
}
