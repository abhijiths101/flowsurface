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
    /// Stores cumulative delta for each timestamp
    data: BTreeMap<u64, f32>,
    /// Running cumulative sum (latest value)
    running_sum: f64,
    /// Last timestamp we've processed
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
                    let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;
                    self.running_sum += delta;
                    self.data.insert(idx as u64, self.running_sum as f32);
                    self.last_time = Some(idx as u64);
                }
            }
        }
        
        self.force_clear_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        for kline in klines {
            let delta = (kline.volume.0 - kline.volume.1) as f64;
            
            // Check if updating existing candle or new candle
            if let Some(existing) = self.data.get(&kline.time) {
                // Update: need to adjust running_sum and recalculate this entry
                // Get the old delta from the difference
                let prev_cum = self.data.range(..kline.time)
                    .next_back()
                    .map(|(_, v)| *v as f64)
                    .unwrap_or(0.0);
                let old_delta = *existing as f64 - prev_cum;
                
                // Adjust running sum by the difference
                let delta_change = delta - old_delta;
                self.running_sum += delta_change;
                
                // Update this entry and all entries after it
                // For simplicity in live updates, we just update this one
                // Full recalc happens in rebuild_from_source for historical
                self.data.insert(kline.time, (prev_cum + delta) as f32);
            } else if self.last_time.map(|t| kline.time > t).unwrap_or(true) {
                // New candle at the end - simple append
                self.running_sum += delta;
                self.data.insert(kline.time, self.running_sum as f32);
                self.last_time = Some(kline.time);
            }
            // else: out of order historical data - ignored, handled by rebuild_from_source
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
            PlotData::TimeBased(_) => {
                // TimeBased: volume updates come through on_insert_klines
            }
            PlotData::TickBased(tick_aggr) => {
                let count = tick_aggr.datapoints.len();
                if count == 0 {
                    return;
                }

                let idx = count - 1;
                let dp = &tick_aggr.datapoints[idx];
                let key = idx as u64;
                let delta = (dp.kline.volume.0 - dp.kline.volume.1) as f64;

                if let Some(existing) = self.data.get(&key) {
                    // Update existing tick candle
                    let prev_cum = if key > 0 {
                        self.data.get(&(key - 1)).copied().unwrap_or(0.0) as f64
                    } else {
                        0.0
                    };
                    let old_delta = *existing as f64 - prev_cum;
                    let delta_change = delta - old_delta;
                    self.running_sum += delta_change;
                    self.data.insert(key, (prev_cum + delta) as f32);
                } else if self.last_time.map(|t| key > t).unwrap_or(true) {
                    // New tick candle
                    self.running_sum += delta;
                    self.data.insert(key, self.running_sum as f32);
                    self.last_time = Some(key);
                }
                
                self.maybe_clear_caches();
            }
        }
    }

    fn on_ticksize_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
}
