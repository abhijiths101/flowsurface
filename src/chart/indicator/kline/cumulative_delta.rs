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
    /// Tracks if we need to rebuild on next cache clear (throttled)
    needs_rebuild: bool,
    /// Source deltas - only rebuilt when needed
    deltas: BTreeMap<u64, f32>,
    last_cache_clear: Instant,
}

impl CumulativeDeltaIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BTreeMap::new(),
            needs_rebuild: false,
            deltas: BTreeMap::new(),
            last_cache_clear: Instant::now(),
        }
    }

    fn maybe_clear_caches(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_cache_clear).as_millis() >= CACHE_THROTTLE_MS {
            // Only rebuild cumulative if needed
            if self.needs_rebuild {
                self.rebuild_cumulative();
                self.needs_rebuild = false;
            }
            self.cache.clear_all();
            self.last_cache_clear = now;
        }
    }

    fn force_clear_caches(&mut self) {
        if self.needs_rebuild {
            self.rebuild_cumulative();
            self.needs_rebuild = false;
        }
        self.cache.clear_all();
        self.last_cache_clear = Instant::now();
    }

    /// Rebuild cumulative data from deltas
    fn rebuild_cumulative(&mut self) {
        self.data.clear();
        let mut running_sum: f64 = 0.0;
        
        for (time, delta) in &self.deltas {
            running_sum += *delta as f64;
            self.data.insert(*time, running_sum as f32);
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
        self.deltas.clear();
        self.data.clear();

        match source {
            PlotData::TimeBased(timeseries) => {
                for (time, dp) in &timeseries.datapoints {
                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    self.deltas.insert(*time, delta);
                }
            }
            PlotData::TickBased(tick_aggr) => {
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    self.deltas.insert(idx as u64, delta);
                }
            }
        }
        
        self.rebuild_cumulative();
        self.needs_rebuild = false;
        self.force_clear_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline]) {
        // Simple and fast - just update deltas, mark for rebuild
        for kline in klines {
            let delta = kline.volume.0 - kline.volume.1;
            self.deltas.insert(kline.time, delta);
        }
        self.needs_rebuild = true;
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
                for (idx, dp) in tick_aggr.datapoints.iter().enumerate() {
                    let delta = dp.kline.volume.0 - dp.kline.volume.1;
                    self.deltas.insert(idx as u64, delta);
                }
                self.needs_rebuild = true;
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
