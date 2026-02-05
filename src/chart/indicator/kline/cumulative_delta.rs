use crate::chart::{
    Caches, Message, ViewState,
    indicator::{
        indicator_row_with_last,
        kline::KlineIndicatorImpl,
        plot::{
            PlotTooltip,
            line::LinePlot,
        },
    },
};

use data::chart::{PlotData, kline::KlineDataPoint};
use data::util::format_with_commas;
use exchange::{Kline, Trade};

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Instant;

const CACHE_THROTTLE_MS: u128 = 200;

pub struct CumulativeDeltaIndicator {
    cache: Caches,
    /// Stores cumulative delta at each timestamp
    cumulative_data: BTreeMap<u64, f32>,
    /// Stores per-candle delta (not cumulative) for recalculation
    per_candle_delta: BTreeMap<u64, f32>,
    last_time: Option<u64>,
    last_cache_clear: Instant,
}

impl CumulativeDeltaIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            cumulative_data: BTreeMap::new(),
            per_candle_delta: BTreeMap::new(),
            last_time: None,
            last_cache_clear: Instant::now(),
        }
    }

    fn maybe_clear_caches(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_cache_clear).as_millis() >= CACHE_THROTTLE_MS {
            self.cache.clear_all();
            self.last_cache_clear = now;
        }
    }

    fn force_clear_caches(&mut self) {
        self.cache.clear_all();
        self.last_cache_clear = Instant::now();
    }

    /// Recalculate cumulative from per-candle deltas starting from a given timestamp
    fn recalculate_cumulative_from(&mut self, from_time: u64) {
        // Get the cumulative value just before from_time
        let mut running_sum: f64 = self.cumulative_data
            .range(..from_time)
            .next_back()
            .map(|(_, v)| *v as f64)
            .unwrap_or(0.0);

        // Recalculate from from_time onwards
        for (time, delta) in self.per_candle_delta.range(from_time..) {
            running_sum += *delta as f64;
            self.cumulative_data.insert(*time, running_sum as f32);
        }
    }

    fn indicator_elem<'a>(
        &'a self,
        main_chart: &'a ViewState,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        let tooltip = |value: &f32, _next: Option<&f32>| {
            PlotTooltip::new(format!("Cum. Delta: {}", format_with_commas(*value)))
        };

        let plot = LinePlot::new(|v: &f32| *v)
            .stroke_width(1.5)
            .show_points(false)
            .with_tooltip(tooltip);

        // Get last CVD value for Y-axis label
        let last_value = self.cumulative_data.values().last().copied().unwrap_or(0.0);

        indicator_row_with_last(main_chart, &self.cache, plot, &self.cumulative_data, visible_range, last_value)
    }
}

impl KlineIndicatorImpl for CumulativeDeltaIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }

    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }

    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        self.indicator_elem(chart, visible_range)
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.cumulative_data.clear();
        self.per_candle_delta.clear();
        self.last_time = None;

        let mut running_sum: f64 = 0.0;

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    self.per_candle_delta.insert(*time, delta);
                    running_sum += delta as f64;
                    self.cumulative_data.insert(*time, running_sum as f32);
                    self.last_time = Some(*time);
                }
            }
            PlotData::TickBased(tick_aggr) => {
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let key = idx as u64;
                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    self.per_candle_delta.insert(key, delta);
                    running_sum += delta as f64;
                    self.cumulative_data.insert(key, running_sum as f32);
                    self.last_time = Some(key);
                }
            }
        }
        self.force_clear_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        let mut earliest_update: Option<u64> = None;

        for kline in klines {
            let delta = kline.volume.0 - kline.volume.1;
            let old_delta = self.per_candle_delta.get(&kline.time).copied();

            // Check if this is an update (delta changed) or new candle
            let is_update = old_delta.map(|old| (old - delta).abs() > 0.001).unwrap_or(false);
            let is_new = old_delta.is_none();

            if is_new || is_update {
                self.per_candle_delta.insert(kline.time, delta);

                // Track earliest time that needs recalculation
                if earliest_update.is_none() || kline.time < earliest_update.unwrap() {
                    earliest_update = Some(kline.time);
                }

                if is_new {
                    self.last_time = Some(kline.time);
                }
            }
        }

        // Recalculate cumulative from the earliest updated candle
        if let Some(from_time) = earliest_update {
            self.recalculate_cumulative_from(from_time);
        }

        self.maybe_clear_caches();
    }

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old_dp_len: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
        match source {
            PlotData::TimeBased(timeseries) => {
                if let Some((time, dp)) = timeseries.datapoints.iter().last() {
                    if *time < self.last_time.unwrap_or(0) {
                        return;
                    }

                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    let is_new = !self.per_candle_delta.contains_key(time);

                    self.per_candle_delta.insert(*time, delta);

                    if is_new {
                        self.last_time = Some(*time);
                    }

                    // Recalculate from this candle
                    self.recalculate_cumulative_from(*time);
                }
            }
            PlotData::TickBased(tick_aggr) => {
                let count = tick_aggr.datapoints.len();
                if count > 0 {
                    let idx = count - 1;
                    let dp = &tick_aggr.datapoints[idx];
                    let key = idx as u64;

                    if key < self.last_time.unwrap_or(0) {
                        return;
                    }

                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    let is_new = !self.per_candle_delta.contains_key(&key);

                    self.per_candle_delta.insert(key, delta);

                    if is_new {
                        self.last_time = Some(key);
                    }

                    // Recalculate from this candle
                    self.recalculate_cumulative_from(key);
                }
            }
        }
        self.maybe_clear_caches();
    }

    fn on_ticksize_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
}
