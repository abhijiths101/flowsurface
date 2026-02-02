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

const SMA_PERIOD: usize = 50;

pub struct SMAIndicator {
    cache: Caches,
    data: BTreeMap<u64, f32>,
    history_closes: Vec<f32>,
    rolling_sum: f64,
    last_time: Option<u64>,
}

impl SMAIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            history_closes: Vec::new(),
            rolling_sum: 0.0,
            last_time: None,
        }
    }

    fn update_rolling(&mut self, new_val: f32, is_new: bool) -> Option<f32> {
        let val_f64 = new_val as f64;
        
        if is_new {
            self.history_closes.push(new_val);
            if self.history_closes.len() > SMA_PERIOD {
                let removed = self.history_closes[self.history_closes.len() - 1 - SMA_PERIOD];
                self.rolling_sum = self.rolling_sum - (removed as f64) + val_f64;
            } else {
                self.rolling_sum += val_f64;
            }
        } else {
            // Updating the last value
            if let Some(last) = self.history_closes.last_mut() {
                let old_val = *last;
                *last = new_val;
                
                // Adjustment: remove old, add new
                self.rolling_sum = self.rolling_sum - (old_val as f64) + val_f64;
            } else {
                // Should not happen if logic is correct
                self.history_closes.push(new_val);
                self.rolling_sum += val_f64;
            }
        }

        if self.history_closes.len() >= SMA_PERIOD {
            Some((self.rolling_sum / SMA_PERIOD as f64) as f32)
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
            PlotTooltip::new(format!("SMA({}): {}", SMA_PERIOD, format_with_commas(*value)))
        };

        let plot = LinePlot::new(|v: &f32| *v)
            .stroke_width(1.5)
            .show_points(false)
            .with_tooltip(tooltip);

        indicator_overlay(main_chart, &self.cache, plot, &self.data, visible_range)
    }
}

impl KlineIndicatorImpl for SMAIndicator {
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
        self.history_closes.clear();
        self.rolling_sum = 0.0;
        self.last_time = None;

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    self.last_time = Some(*time);
                    if let Some(sma) = self.update_rolling(dp.kline.close.to_f32(), true) {
                        self.data.insert(*time, sma);
                    }
                }
            }
            PlotData::TickBased(tick_aggr) => {
                 for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    // TickBased uses index as time/key
                    let key = idx as u64;
                    self.last_time = Some(key); // Using index as "time" for tracking
                    if let Some(sma) = self.update_rolling(dp.kline.close.to_f32(), true) {
                        self.data.insert(key, sma);
                    }
                 }
            }
        }
        self.clear_all_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        for kline in klines {
            // Chronological check?
            if let Some(last) = self.last_time {
                if kline.time <= last {
                    // Out of order data. Skipping or forcing rebuild would be safer.
                    // For now, ignoring to prevent corruption.
                    continue; 
                }
            }
            self.last_time = Some(kline.time);
            if let Some(sma) = self.update_rolling(kline.close.to_f32(), true) {
                self.data.insert(kline.time, sma);
            }
        }
        self.clear_all_caches();
    }

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old_dp_len: usize, // Unused now that we rely on last_time
        source: &PlotData<KlineDataPoint>,
    ) {
         match source {
            PlotData::TimeBased(timeseries) => {
                if let Some((time, dp)) = timeseries.datapoints.iter().last() {
                     let is_new = match self.last_time {
                         Some(last) => *time > last,
                         None => true,
                     };
                     
                     // If *time < last, it's an update to an old candle? 
                     // Or just weird behavior. We assume strictly increasing or same.
                     if *time < self.last_time.unwrap_or(0) {
                         return; // Ignore updates to past
                     }
                     
                     self.last_time = Some(*time);

                     if let Some(sma) = self.update_rolling(dp.kline.close.to_f32(), is_new) {
                        self.data.insert(*time, sma);
                     }
                }
            },
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
                     
                     self.last_time = Some(key);
                     
                     if let Some(sma) = self.update_rolling(dp.kline.close.to_f32(), is_new) {
                        self.data.insert(key, sma);
                     }
                 }
            }
        }
        self.clear_all_caches();
    }

    fn on_ticksize_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
}
