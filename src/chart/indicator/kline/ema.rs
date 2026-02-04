use crate::chart::{
    Caches, Message, ViewState,
    indicator::{
        indicator_overlay,
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

const EMA_PERIOD: usize = 20;
const CACHE_THROTTLE_MS: u128 = 200;

pub struct EMAIndicator {
    cache: Caches,
    data: BTreeMap<u64, f32>,
    last_ema: Option<f32>,
    multiplier: f32,
    history_len: usize,
    last_cache_clear: Instant,
}

impl EMAIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            last_ema: None,
            multiplier: 2.0 / (EMA_PERIOD as f32 + 1.0),
            history_len: 0,
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

    fn calculate_next_ema(&self, price: f32, prev_ema: f32) -> f32 {
        (price - prev_ema) * self.multiplier + prev_ema
    }

    fn indicator_elem<'a>(
        &'a self,
        main_chart: &'a ViewState,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        let tooltip = |value: &f32, _next: Option<&f32>| {
            PlotTooltip::new(format!("EMA({}): {}", EMA_PERIOD, format_with_commas(*value)))
        };

        let plot = LinePlot::new(|v: &f32| *v)
            .stroke_width(1.5)
            .show_points(false)
            .with_tooltip(tooltip);

        indicator_overlay(main_chart, &self.cache, plot, &self.data, visible_range)
    }
}

impl KlineIndicatorImpl for EMAIndicator {
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
        self.last_ema = None;
        self.history_len = 0;

        // Collect first N items
        // For first `EMA_PERIOD` items, we can use SMA as seed, or just start accumulation.
        // Standard behavior: Use SMA of first N items as the initial EMA point.
        let mut initial_sum = 0.0;
        let mut count = 0;

        match source {
            PlotData::TimeBased(timeseries) => {
                 for (time, dp) in &timeseries.datapoints {
                    self.history_len += 1;
                    if count < EMA_PERIOD {
                        initial_sum += dp.kline.close.to_f32();
                        count += 1;
                        if count == EMA_PERIOD {
                            let sma = initial_sum / EMA_PERIOD as f32;
                            self.last_ema = Some(sma);
                            self.data.insert(*time, sma);
                        }
                    } else {
                         let next = self.calculate_next_ema(dp.kline.close.to_f32(), self.last_ema.unwrap());
                         self.last_ema = Some(next);
                         self.data.insert(*time, next);
                    }
                }
            }
            PlotData::TickBased(tick_aggr) => {
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    self.history_len += 1;
                    if count < EMA_PERIOD {
                        initial_sum += dp.kline.close.to_f32();
                        count += 1;
                        if count == EMA_PERIOD {
                            let sma = initial_sum / EMA_PERIOD as f32;
                            self.last_ema = Some(sma);
                            self.data.insert(idx as u64, sma);
                        }
                    } else {
                         let next = self.calculate_next_ema(dp.kline.close.to_f32(), self.last_ema.unwrap());
                         self.last_ema = Some(next);
                         self.data.insert(idx as u64, next);
                    }
                }
            }
        }
        
        self.force_clear_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        for kline in klines {
            self.history_len += 1;
             if self.history_len <= EMA_PERIOD {
                 // Rebuild if we are just crossing the threshold to be safe/simple
                 // Ideally we accumulate sum but we discarded it. 
                 // Simple approach: if history is small, simple rebuild is cheap.
                 // But we don't have access to source. 
                 // Assuming we are receiving klines in order.
                 // If we haven't initialized last_ema yet, we can't do much without full history.
                 // BUT: This function is usually called for NEW klines arriving live.
                 // So we usually already have history.
            } else if let Some(prev) = self.last_ema {
                let next = self.calculate_next_ema(kline.close.to_f32(), prev);
                self.last_ema = Some(next);
                self.data.insert(kline.time, next);
            }
        }
        self.maybe_clear_caches();
    }

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        old_dp_len: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
         match source {
            PlotData::TimeBased(timeseries) => {
                // Update last candle
                if let Some((time, dp)) = timeseries.datapoints.iter().last() {
                     // We need the EMA *before* this last candle to recalculate.
                     // Since we store computed EMAs in `self.data`, we can look up `prev` one.
                     // But `BTreeMap` doesn't give random access easily by index.
                     // `timeseries` keys are ordered.
                     
                     // Optimization: if we have keys A, B, C... 
                     // and we are updating C. We need B's EMA value.
                     
                     // If this is a new candle (not in data yet?), no, `on_insert_trades` updates existing bucket.
                     // So `time` is already in `self.data`? 
                     
                     let prev_ema = if let Some((prev_time, _)) = timeseries.datapoints.range(..*time).next_back() {
                         self.data.get(prev_time).copied()
                     } else {
                         None 
                     };
                     
                     if let Some(prev) = prev_ema {
                         let next = self.calculate_next_ema(dp.kline.close.to_f32(), prev);
                         self.last_ema = Some(next);
                         self.data.insert(*time, next);
                     }
                }
            },
            PlotData::TickBased(tick_aggr) => {
                let current_len = tick_aggr.datapoints.len();
                if current_len > old_dp_len {
                    // New bars added
                     for (i, dp) in tick_aggr.datapoints.iter().enumerate().skip(old_dp_len) {
                        self.history_len += 1;
                        // Need prev ema.
                         let prev_ema = if i > 0 { self.data.get(&((i - 1) as u64)).copied() } else { None };
                         
                         if let Some(prev) = prev_ema {
                             let next = self.calculate_next_ema(dp.kline.close.to_f32(), prev);
                             self.last_ema = Some(next);
                             self.data.insert(i as u64, next);
                         } 
                         // Note: If we are still building initial Period, this logic skips. 
                         // That's acceptable for now (live stabilization).
                    }
                } else if !tick_aggr.datapoints.is_empty() {
                    // Update last one
                     let last_idx = current_len - 1;
                     let dp = &tick_aggr.datapoints[last_idx];
                     
                     let prev_ema = if last_idx > 0 { self.data.get(&((last_idx - 1) as u64)).copied() } else { None };
                     
                     if let Some(prev) = prev_ema {
                         let next = self.calculate_next_ema(dp.kline.close.to_f32(), prev);
                         self.last_ema = Some(next);
                         self.data.insert(last_idx as u64, next);
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
