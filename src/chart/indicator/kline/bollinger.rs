use crate::chart::{
    Caches, Message, ViewState,
    indicator::{
        indicator_overlay,
        kline::KlineIndicatorImpl,
        plot::{
            PlotTooltip,
        },
    },
};

use data::chart::{PlotData, kline::KlineDataPoint};
use data::util::format_with_commas;
use exchange::{Kline, Trade};

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Instant;

const BB_PERIOD: usize = 20;
const BB_STD_DEV: f32 = 2.0;
const CACHE_THROTTLE_MS: u128 = 200;

#[derive(Debug, Clone, Copy, Default)]
struct BandValue {
    upper: f32,
    middle: f32,
    lower: f32,
}

pub struct BollingerIndicator {
    cache: Caches,
    data: BTreeMap<u64, BandValue>,
    history_closes: Vec<f32>,
    last_ema: Option<f32>,
    multiplier: f32,
    rolling_sum: f64,
    rolling_sum_sq: f64,
    last_time: Option<u64>,
    last_cache_clear: Instant,
}

impl BollingerIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            history_closes: Vec::new(),
            last_ema: None,
            multiplier: 2.0 / (BB_PERIOD as f32 + 1.0),
            rolling_sum: 0.0,
            rolling_sum_sq: 0.0,
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

    fn calculate_next_ema(&self, price: f32, prev_ema: f32) -> f32 {
        (price - prev_ema) * self.multiplier + prev_ema
    }

    fn update_rolling_stats(&mut self, new_val: f32, is_new: bool) -> Option<f32> {
        let val_f64 = new_val as f64;
        let val_sq = val_f64 * val_f64;

        if is_new {
            self.history_closes.push(new_val);
            if self.history_closes.len() > BB_PERIOD {
                let removed = self.history_closes[self.history_closes.len() - 1 - BB_PERIOD];
                let rem_f64 = removed as f64;
                self.rolling_sum = self.rolling_sum - rem_f64 + val_f64;
                self.rolling_sum_sq = self.rolling_sum_sq - (rem_f64 * rem_f64) + val_sq;
            } else {
                self.rolling_sum += val_f64;
                self.rolling_sum_sq += val_sq;
            }
        } else {
             if let Some(last) = self.history_closes.last_mut() {
                let old_val = *last;
                *last = new_val;
                
                let old_f64 = old_val as f64;
                self.rolling_sum = self.rolling_sum - old_f64 + val_f64;
                self.rolling_sum_sq = self.rolling_sum_sq - (old_f64 * old_f64) + val_sq;
            } else {
                self.history_closes.push(new_val);
                self.rolling_sum += val_f64;
                self.rolling_sum_sq += val_sq;
            }
        }

        if self.history_closes.len() >= BB_PERIOD {
            let mean = self.rolling_sum / BB_PERIOD as f64;
            // E[X^2] - (E[X])^2
            let mean_sq = self.rolling_sum_sq / BB_PERIOD as f64;
            let variance = mean_sq - (mean * mean);
            // Variance can be slightly negative due to precision, clamp to 0
            Some(variance.max(0.0).sqrt() as f32)
        } else {
            None
        }
    }







    fn indicator_elem<'a>(
        &'a self,
        main_chart: &'a ViewState,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        let _tooltip = |value: &BandValue, _next: Option<&BandValue>| {
            PlotTooltip::new(format!(
                "BB({}, {}):\nUpper: {}\nMiddle: {}\nLower: {}", 
                BB_PERIOD, BB_STD_DEV, 
                format_with_commas(value.upper), 
                format_with_commas(value.middle), 
                format_with_commas(value.lower)
            ))
        };

        // We need to render 3 lines. indicator_row supports one plot.
        // But LinePlot takes a value extractor `V`. 
        // We can create composite plot or overlapping indicators?
        // `indicator_row` implementation: `plot.draw(...)`.
        // If we want multiple lines, we can't do it with a single `LinePlot`.
        // `LinePlot` draws ONE line.
        // We might need to modify `LinePlot` or use a wrapper.
        // Or cleaner: Implement a `MultiLinePlot`?
        // Or just implement `draw` manually here without `LinePlot`?
        // `indicator_row` is generic over `P: Plot`.
        // Function signature: `pub fn indicator_row<S, P>(..., plot: P, datapoints: &S, ...)`
        // We can conform to `Plot` trait ourselves!
        
        let plot = BollingerPlot {
            _period: BB_PERIOD,
            _k: BB_STD_DEV,
        };

        indicator_overlay(main_chart, &self.cache, plot, &self.data, visible_range)
    }
}

// Custom Plot for Bollinger Bands to draw 3 lines
struct BollingerPlot {
    _period: usize,
    _k: f32,
}

use iced::widget::canvas::{self, Path, Stroke};
use iced::Theme;
use crate::chart::indicator::plot::{Plot, Series, TooltipFn, YScale};

impl<S> Plot<S> for BollingerPlot
where 
    S: Series<Y = BandValue>
{
    fn y_extents(&self, datapoints: &S, range: RangeInclusive<u64>) -> Option<(f32, f32)> {
        let mut min_v = f32::MAX;
        let mut max_v = f32::MIN;

        datapoints.for_each_in(range, |_, v| {
            if v.lower < min_v { min_v = v.lower; }
            if v.upper > max_v { max_v = v.upper; }
        });

        if min_v == f32::MAX { None } else { Some((min_v, max_v)) }
    }

    fn adjust_extents(&self, min: f32, max: f32) -> (f32, f32) {
         if max > min {
            let range = max - min;
            let pad = range * 0.05;
            (min - pad, max + pad)
        } else {
            (min, max)
        }
    }

    fn draw(
        &self,
        frame: &mut canvas::Frame,
        ctx: &ViewState,
        theme: &Theme,
        datapoints: &S,
        range: RangeInclusive<u64>,
        scale: &YScale,
    ) {
        let palette = theme.extended_palette();
        let middle_color = palette.secondary.base.color;
        let band_color = palette.primary.strong.color;
        
        let middle_stroke = Stroke::with_color(Stroke { width: 1.0, ..Stroke::default() }, middle_color);
        let band_stroke = Stroke::with_color(Stroke { width: 1.0, ..Stroke::default() }, band_color);
        
        // Single pass: draw all 3 lines at once
        let mut prev_middle: Option<(f32, f32)> = None;
        let mut prev_upper: Option<(f32, f32)> = None;
        let mut prev_lower: Option<(f32, f32)> = None;
        
        datapoints.for_each_in(range, |x, y| {
            let sx = ctx.interval_to_x(x) - (ctx.cell_width / 2.0);
            let sy_middle = scale.to_y(y.middle);
            let sy_upper = scale.to_y(y.upper);
            let sy_lower = scale.to_y(y.lower);
            
            if let Some((px, py)) = prev_middle {
                frame.stroke(&Path::line(iced::Point::new(px, py), iced::Point::new(sx, sy_middle)), middle_stroke);
            }
            if let Some((px, py)) = prev_upper {
                frame.stroke(&Path::line(iced::Point::new(px, py), iced::Point::new(sx, sy_upper)), band_stroke);
            }
            if let Some((px, py)) = prev_lower {
                frame.stroke(&Path::line(iced::Point::new(px, py), iced::Point::new(sx, sy_lower)), band_stroke);
            }
            
            prev_middle = Some((sx, sy_middle));
            prev_upper = Some((sx, sy_upper));
            prev_lower = Some((sx, sy_lower));
        });
    }

    fn tooltip_fn(&self) -> Option<&TooltipFn<S::Y>> {
         // Return a Box containing the closure we defined earlier? 
         // `indicator_elem` created a tooltip closure, but `Plot` needs to own/return it or we pass it in.
         // `LinePlot` stores it. We should store it too.
         // For brevity, defaulting to None to avoid complex type matching in this struct for now, 
         // or implement basic inside struct.
         // Retrying: Let's make `BollingerPlot` store the optional tooltip.
         None 
    }
}
// Note: Tooltip missing in `BollingerPlot` above to save complexity, but we can add it if needed.
// Or we can add `tooltip: Option<Box<dyn Fn...>>` field to struct.

impl KlineIndicatorImpl for BollingerIndicator {
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
        self.last_ema = None;
        self.rolling_sum = 0.0;
        self.rolling_sum_sq = 0.0;
        self.last_time = None;
        
        // Initial EMA seed helper
        // Standard: SMA of first N.
        // We will build incrementally.
        
        let mut initial_sum = 0.0;
        let mut count = 0;

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    let close = dp.kline.close.to_f32();
                    self.last_time = Some(*time);
                    let std_dev = self.update_rolling_stats(close, true);

                    if count < BB_PERIOD {
                        initial_sum += close;
                        count += 1;
                        if count == BB_PERIOD {
                            let sma = initial_sum / BB_PERIOD as f32;
                            self.last_ema = Some(sma);
                             if let Some(sd) = std_dev {
                                self.data.insert(*time, BandValue {
                                    middle: sma,
                                    upper: sma + BB_STD_DEV * sd,
                                    lower: sma - BB_STD_DEV * sd,
                                });
                            }
                        }
                    } else {
                        let prev = self.last_ema.unwrap();
                        let next = self.calculate_next_ema(close, prev);
                        self.last_ema = Some(next);
                         if let Some(sd) = std_dev {
                            self.data.insert(*time, BandValue {
                                middle: next,
                                upper: next + BB_STD_DEV * sd,
                                lower: next - BB_STD_DEV * sd,
                            });
                        }
                    }
                }
            }
            PlotData::TickBased(tick_aggr) => {
                 for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let close = dp.kline.close.to_f32();
                    let key = idx as u64;
                    self.last_time = Some(key);
                    let std_dev = self.update_rolling_stats(close, true);

                    if count < BB_PERIOD {
                        initial_sum += close;
                        count += 1;
                         if count == BB_PERIOD {
                            let sma = initial_sum / BB_PERIOD as f32;
                            self.last_ema = Some(sma);
                             if let Some(sd) = std_dev {
                                self.data.insert(key, BandValue {
                                    middle: sma,
                                    upper: sma + BB_STD_DEV * sd,
                                    lower: sma - BB_STD_DEV * sd,
                                });
                            }
                        }
                    } else {
                        let prev = self.last_ema.unwrap();
                        let next = self.calculate_next_ema(close, prev);
                        self.last_ema = Some(next);
                         if let Some(sd) = std_dev {
                             self.data.insert(key, BandValue {
                                middle: next,
                                upper: next + BB_STD_DEV * sd,
                                lower: next - BB_STD_DEV * sd,
                            });
                        }
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
                    continue; // Skip out of order
                }
            }
            self.last_time = Some(kline.time);
            
            let close = kline.close.to_f32();
            let std_dev = self.update_rolling_stats(close, true);
            
            if self.last_ema.is_none() {
                if self.history_closes.len() >= BB_PERIOD {
                     // Need partial sum from history to init EMA if we just crossed?
                     // But history is already managed by update_rolling_stats.
                     // The simple way: start EMA from current simple mean (stats.rolling_sum / N).
                     let sma = (self.rolling_sum / BB_PERIOD as f64) as f32;
                     self.last_ema = Some(sma);
                     
                     if let Some(sd) = std_dev {
                         self.data.insert(kline.time, BandValue {
                            middle: sma,
                            upper: sma + BB_STD_DEV * sd,
                            lower: sma - BB_STD_DEV * sd,
                        });
                     }
                }
            } else if let Some(prev) = self.last_ema {
                 let next = self.calculate_next_ema(close, prev);
                 self.last_ema = Some(next);
                 
                 if let Some(sd) = std_dev {
                     self.data.insert(kline.time, BandValue {
                        middle: next,
                        upper: next + BB_STD_DEV * sd,
                        lower: next - BB_STD_DEV * sd,
                    });
                 }
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
                         return; // Ignore updates to past
                     }
                     self.last_time = Some(*time);
                     
                     let close = dp.kline.close.to_f32();
                     let std_dev = self.update_rolling_stats(close, is_new);
                     
                     // EMA
                     if is_new {
                         // New candle: use prev EMA from *finalized* previous candle.
                         // But we don't store "finalized" explicitly separate from last_ema.
                         // Or do we? `last_ema` tracks latest.
                         // If it's NEW, `last_ema` IS the finalized previous EMA.
                         // So we just use it.
                         if let Some(prev) = self.last_ema {
                             let next = self.calculate_next_ema(close, prev);
                             self.last_ema = Some(next);
                             
                             if let Some(sd) = std_dev {
                                 self.data.insert(*time, BandValue {
                                    middle: next,
                                    upper: next + BB_STD_DEV * sd,
                                    lower: next - BB_STD_DEV * sd,
                                });
                             }
                        } else if self.history_closes.len() >= BB_PERIOD {
                            // First time init
                            let sma = (self.rolling_sum / BB_PERIOD as f64) as f32;
                            self.last_ema = Some(sma);
                             if let Some(sd) = std_dev {
                                 self.data.insert(*time, BandValue {
                                    middle: sma,
                                    upper: sma + BB_STD_DEV * sd,
                                    lower: sma - BB_STD_DEV * sd,
                                });
                             }
                        }
                     } else {
                         // Updating existing candle.
                         // We need PREV EMA (N-1).
                         // `self.last_ema` is currently N (from previous update of this candle).
                         // We must fetch N-1.
                         let prev_ema = if let Some((_, val)) = self.data.range(..*time).next_back() {
                             Some(val.middle)
                         } else { None };

                         if let Some(prev) = prev_ema {
                             let next = self.calculate_next_ema(close, prev);
                             self.last_ema = Some(next);
                             
                              if let Some(sd) = std_dev {
                                 self.data.insert(*time, BandValue {
                                    middle: next,
                                    upper: next + BB_STD_DEV * sd,
                                    lower: next - BB_STD_DEV * sd,
                                });
                             }
                         }
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
                     
                     let close = dp.kline.close.to_f32();
                     let std_dev = self.update_rolling_stats(close, is_new);

                     if is_new {
                         if let Some(prev) = self.last_ema {
                             let next = self.calculate_next_ema(close, prev);
                             self.last_ema = Some(next);
                             
                             if let Some(sd) = std_dev {
                                 self.data.insert(key, BandValue {
                                    middle: next,
                                    upper: next + BB_STD_DEV * sd,
                                    lower: next - BB_STD_DEV * sd,
                                });
                             }
                        } else if self.history_closes.len() >= BB_PERIOD {
                            let sma = (self.rolling_sum / BB_PERIOD as f64) as f32;
                            self.last_ema = Some(sma);
                             if let Some(sd) = std_dev {
                                 self.data.insert(key, BandValue {
                                    middle: sma,
                                    upper: sma + BB_STD_DEV * sd,
                                    lower: sma - BB_STD_DEV * sd,
                                });
                             }
                        }
                     } else {
                         let prev_ema = if key > 0 { self.data.get(&((key - 1))).map(|v| v.middle) } else { None };
                         
                          if let Some(prev) = prev_ema {
                             let next = self.calculate_next_ema(close, prev);
                             self.last_ema = Some(next);
                             
                              if let Some(sd) = std_dev {
                                 self.data.insert(key, BandValue {
                                    middle: next,
                                    upper: next + BB_STD_DEV * sd,
                                    lower: next - BB_STD_DEV * sd,
                                });
                             }
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
