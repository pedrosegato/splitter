pub struct SineGenerator {
    phase: f32,
    frequency: f32,
    sample_rate: f32,
}

impl SineGenerator {
    pub fn new(frequency_hz: f32) -> Self {
        Self {
            phase: 0.0,
            frequency: frequency_hz,
            sample_rate: crate::SAMPLE_RATE as f32,
        }
    }

    pub fn fill(&mut self, buf: &mut [f32]) {
        let delta = 2.0 * std::f32::consts::PI * self.frequency / self.sample_rate;
        for s in buf.iter_mut() {
            *s = self.phase.sin() * 0.5;
            self.phase = (self.phase + delta) % (2.0 * std::f32::consts::PI);
        }
    }
}
