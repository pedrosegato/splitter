use crate::settings::FecMode;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FecSetting {
    pub enable: bool,
    pub packet_loss_perc: u8,
}

#[derive(Debug)]
pub struct FecController {
    mode: FecMode,
    on_threshold_pct: u32,
    off_threshold_pct: u32,
    hysteresis: Duration,
    window: Duration,
    samples: VecDeque<(Instant, bool)>,
    enabled: bool,
    last_flip: Option<Instant>,
}

impl FecController {
    pub fn new(
        mode: FecMode,
        on_threshold_pct: u32,
        off_threshold_pct: u32,
        hysteresis_secs: u32,
    ) -> Self {
        Self {
            mode,
            on_threshold_pct,
            off_threshold_pct,
            hysteresis: Duration::from_secs(hysteresis_secs as u64),
            window: Duration::from_secs(5),
            samples: VecDeque::with_capacity(512),
            enabled: false,
            last_flip: None,
        }
    }

    pub fn record(&mut self, now: Instant, lost: bool) {
        self.samples.push_back((now, lost));
        while let Some((t, _)) = self.samples.front() {
            if now.duration_since(*t) > self.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn current_loss_pct(&self) -> u32 {
        if self.samples.is_empty() {
            return 0;
        }
        let lost = self.samples.iter().filter(|(_, l)| *l).count() as u32;
        (lost * 100) / self.samples.len() as u32
    }

    pub fn evaluate(&mut self, now: Instant) -> FecSetting {
        let pct = self.current_loss_pct();
        let desired = match self.mode {
            FecMode::Always => true,
            FecMode::Never => false,
            FecMode::Auto => {
                if self.enabled {
                    pct > self.off_threshold_pct
                } else {
                    pct > self.on_threshold_pct
                }
            }
        };
        if desired != self.enabled {
            let allow_flip = match self.last_flip {
                None => true,
                Some(t) => now.duration_since(t) >= self.hysteresis,
            };
            if allow_flip {
                self.enabled = desired;
                self.last_flip = Some(now);
                tracing::info!(enabled = self.enabled, loss_pct = pct, "FEC state flipped");
            }
        }
        FecSetting {
            enable: self.enabled,
            packet_loss_perc: pct.min(100) as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        controller: &mut FecController,
        now: Instant,
        lost_count: usize,
        total: usize,
    ) -> FecSetting {
        for i in 0..total {
            controller.record(now, i < lost_count);
        }
        controller.evaluate(now)
    }

    #[test]
    fn always_mode_keeps_fec_on() {
        let mut c = FecController::new(FecMode::Always, 1, 0, 10);
        let s = step(&mut c, Instant::now(), 0, 100);
        assert!(s.enable);
    }

    #[test]
    fn never_mode_keeps_fec_off_even_with_high_loss() {
        let mut c = FecController::new(FecMode::Never, 1, 0, 10);
        let s = step(&mut c, Instant::now(), 50, 100);
        assert!(!s.enable);
    }

    #[test]
    fn auto_enables_above_threshold() {
        let mut c = FecController::new(FecMode::Auto, 1, 0, 0);
        let s = step(&mut c, Instant::now(), 5, 100);
        assert!(s.enable);
    }

    #[test]
    fn auto_stays_off_below_threshold() {
        let mut c = FecController::new(FecMode::Auto, 1, 0, 0);
        let s = step(&mut c, Instant::now(), 0, 100);
        assert!(!s.enable);
    }

    #[test]
    fn hysteresis_blocks_quick_reflips() {
        let mut c = FecController::new(FecMode::Auto, 1, 0, 10);
        let t0 = Instant::now();
        let on = step(&mut c, t0, 5, 100);
        assert!(on.enable);
        let t1 = t0;
        for _ in 0..200 {
            c.record(t1, false);
        }
        let still = c.evaluate(t1);
        assert!(still.enable, "should be blocked by hysteresis");
    }

    #[test]
    fn loss_pct_drops_to_zero_outside_window() {
        let mut c = FecController::new(FecMode::Auto, 1, 0, 0);
        let t0 = Instant::now();
        for _ in 0..100 {
            c.record(t0, true);
        }
        assert!(c.current_loss_pct() > 0);
        let t_later = t0 + Duration::from_secs(10);
        c.record(t_later, false);
        assert!(c.current_loss_pct() < 100);
    }
}
