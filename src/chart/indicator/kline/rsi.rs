use crate::chart::{
    Caches, Message, ViewState,
    indicator::{
        indicator_row,
        kline::KlineIndicatorImpl,
        plot::{
            PlotTooltip,
            line::LinePlot,
        },
    },
};

use data::chart::{PlotData, kline::KlineDataPoint};
use exchange::{Kline, Trade};

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Instant;

const RSI_PERIOD: usize = 14;
const CACHE_THROTTLE_MS: u128 = 200;

pub struct RSIIndicator {
    cache: Caches,
    data: BTreeMap<u64, f32>,
    last_close: Option<f32>,
    finalized_avg_gain: Option<f64>,
    finalized_avg_loss: Option<f64>,
    candle_count: usize,
    init_gain_sum: f64,
    init_loss_sum: f64,
    last_time: Option<u64>,
    last_cache_clear: Instant,
}

impl RSIIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            last_close: None,
            finalized_avg_gain: None,
            finalized_avg_loss: None,
            candle_count: 0,
            init_gain_sum: 0.0,
            init_loss_sum: 0.0,
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

    /// Process a new candle close, returns RSI if ready
    fn process_new_candle(&mut self, close: f32) -> Option<f32> {
        self.candle_count += 1;
        
        if let Some(prev) = self.last_close {
            let change = (close - prev) as f64;
            let (gain, loss) = if change > 0.0 {
                (change, 0.0)
            } else {
                (0.0, -change)
            };

            if self.candle_count <= RSI_PERIOD {
                // Accumulating for initial SMA
                self.init_gain_sum += gain;
                self.init_loss_sum += loss;
                
                if self.candle_count == RSI_PERIOD {
                    // Initialize with SMA
                    self.finalized_avg_gain = Some(self.init_gain_sum / RSI_PERIOD as f64);
                    self.finalized_avg_loss = Some(self.init_loss_sum / RSI_PERIOD as f64);
                }
            } else if let (Some(avg_gain), Some(avg_loss)) = (self.finalized_avg_gain, self.finalized_avg_loss) {
                // Wilder's smoothing
                let period = RSI_PERIOD as f64;
                let new_avg_gain = (avg_gain * (period - 1.0) + gain) / period;
                let new_avg_loss = (avg_loss * (period - 1.0) + loss) / period;
                
                self.finalized_avg_gain = Some(new_avg_gain);
                self.finalized_avg_loss = Some(new_avg_loss);
            }
        }
        
        self.last_close = Some(close);
        self.calc_current_rsi()
    }

    /// Update current candle (not finalized), returns RSI
    fn update_current_candle(&mut self, close: f32) -> Option<f32> {
        // For live updates, we calculate tentative RSI without modifying finalized state
        if let (Some(prev), Some(avg_gain), Some(avg_loss)) = 
            (self.last_close, self.finalized_avg_gain, self.finalized_avg_loss) 
        {
            let change = (close - prev) as f64;
            let (gain, loss) = if change > 0.0 {
                (change, 0.0)
            } else {
                (0.0, -change)
            };

            let period = RSI_PERIOD as f64;
            let tentative_gain = (avg_gain * (period - 1.0) + gain) / period;
            let tentative_loss = (avg_loss * (period - 1.0) + loss) / period;

            let rsi = if tentative_loss == 0.0 {
                100.0
            } else {
                let rs = tentative_gain / tentative_loss;
                100.0 - (100.0 / (1.0 + rs))
            };

            return Some(rsi as f32);
        }
        None
    }

    fn calc_current_rsi(&self) -> Option<f32> {
        if let (Some(avg_gain), Some(avg_loss)) = (self.finalized_avg_gain, self.finalized_avg_loss) {
            let rsi = if avg_loss == 0.0 {
                100.0
            } else {
                let rs = avg_gain / avg_loss;
                100.0 - (100.0 / (1.0 + rs))
            };
            Some(rsi as f32)
        } else {
            None
        }
    }

    fn indicator_elem<'a>(
        &'a self,
        main_chart: &'a ViewState,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        let tooltip = |value: &f32, _next: Option<&f32>| {
            PlotTooltip::new(format!("RSI({}): {:.2}", RSI_PERIOD, value))
        };

        let plot = LinePlot::new(|v: &f32| *v)
            .stroke_width(1.5)
            .show_points(false)
            .with_tooltip(tooltip);

        indicator_row(main_chart, &self.cache, plot, &self.data, visible_range)
    }
}

impl KlineIndicatorImpl for RSIIndicator {
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
        self.last_close = None;
        self.finalized_avg_gain = None;
        self.finalized_avg_loss = None;
        self.candle_count = 0;
        self.init_gain_sum = 0.0;
        self.init_loss_sum = 0.0;
        self.last_time = None;

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    self.last_time = Some(*time);
                    if let Some(rsi) = self.process_new_candle(dp.kline.close.to_f32()) {
                        self.data.insert(*time, rsi);
                    }
                }
            }
            PlotData::TickBased(tick_aggr) => {
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let key = idx as u64;
                    self.last_time = Some(key);
                    if let Some(rsi) = self.process_new_candle(dp.kline.close.to_f32()) {
                        self.data.insert(key, rsi);
                    }
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
            if let Some(rsi) = self.process_new_candle(kline.close.to_f32()) {
                self.data.insert(kline.time, rsi);
            }
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

                    let close = dp.kline.close.to_f32();
                    
                    if is_new {
                        self.last_time = Some(*time);
                        if let Some(rsi) = self.process_new_candle(close) {
                            self.data.insert(*time, rsi);
                        }
                    } else {
                        // Update current candle without modifying finalized state
                        if let Some(rsi) = self.update_current_candle(close) {
                            self.data.insert(*time, rsi);
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

                    let close = dp.kline.close.to_f32();

                    if is_new {
                        self.last_time = Some(key);
                        if let Some(rsi) = self.process_new_candle(close) {
                            self.data.insert(key, rsi);
                        }
                    } else {
                        if let Some(rsi) = self.update_current_candle(close) {
                            self.data.insert(key, rsi);
                        }
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
