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
    data: BTreeMap<u64, f32>,
    running_sum: f64,
    last_time: Option<u64>,
    last_cache_clear: Instant,
}

impl CumulativeDeltaIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            running_sum: 0.0,
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
        let last_value = self.data.values().last().copied().unwrap_or(0.0);

        indicator_row_with_last(main_chart, &self.cache, plot, &self.data, visible_range, last_value)
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
        self.data.clear();
        self.running_sum = 0.0;
        self.last_time = None;

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;
                    self.running_sum += delta;
                    self.data.insert(*time, self.running_sum as f32);
                    self.last_time = Some(*time);
                }
            }
            PlotData::TickBased(tick_aggr) => {
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let key = idx as u64;
                    let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;
                    self.running_sum += delta;
                    self.data.insert(key, self.running_sum as f32);
                    self.last_time = Some(key);
                }
            }
        }
        self.force_clear_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        for kline in klines {
            if let Some(last) = self.last_time {
                if kline.time <= last {
                    continue;
                }
            }
            self.last_time = Some(kline.time);
            let delta = (kline.volume.0 - kline.volume.1) as f64;
            self.running_sum += delta;
            self.data.insert(kline.time, self.running_sum as f32);
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
                    let is_new = match self.last_time {
                        Some(last) => *time > last,
                        None => true,
                    };

                    if *time < self.last_time.unwrap_or(0) {
                        return;
                    }

                    let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;

                    if is_new {
                        self.last_time = Some(*time);
                        self.running_sum += delta;
                        self.data.insert(*time, self.running_sum as f32);
                    } else {
                        // Update current candle: recalculate from previous sum
                        if let Some((&prev_time, &prev_val)) = self.data.range(..*time).next_back() {
                            let _ = prev_time; // silence unused
                            let new_cum = prev_val as f64 + delta;
                            self.data.insert(*time, new_cum as f32);
                            self.running_sum = new_cum;
                        } else {
                            // First candle update
                            self.data.insert(*time, delta as f32);
                            self.running_sum = delta;
                        }
                    }
                }
            }
            PlotData::TickBased(tick_aggr) => {
                let count = tick_aggr.datapoints.len();
                if count > 0 {
                    let idx = count - 1;
                    let dp = &tick_aggr.datapoints[idx];
                    let key = idx as u64;

                    let is_new = match self.last_time {
                        Some(last) => key > last,
                        None => true,
                    };

                    if key < self.last_time.unwrap_or(0) {
                        return;
                    }

                    let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;

                    if is_new {
                        self.last_time = Some(key);
                        self.running_sum += delta;
                        self.data.insert(key, self.running_sum as f32);
                    } else {
                        // Update current candle
                        let prev_val = if key > 0 {
                            self.data.get(&(key - 1)).copied().unwrap_or(0.0)
                        } else {
                            0.0
                        };
                        let new_cum = prev_val as f64 + delta;
                        self.data.insert(key, new_cum as f32);
                        self.running_sum = new_cum;
                    }
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
