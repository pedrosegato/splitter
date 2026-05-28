use crate::error::AudioError;
use std::marker::PhantomData;

pub struct RtPriorityHandle {
    inner: Option<audio_thread_priority::RtPriorityHandle>,
    _not_send: PhantomData<*const ()>,
}

pub fn promote_current_thread() -> Result<RtPriorityHandle, AudioError> {
    match audio_thread_priority::promote_current_thread_to_real_time(0, crate::SAMPLE_RATE) {
        Ok(handle) => Ok(RtPriorityHandle {
            inner: Some(handle),
            _not_send: PhantomData,
        }),
        Err(e) => {
            tracing::warn!("audio thread priority promotion failed: {e}");
            Err(promotion_error(e))
        }
    }
}

fn promotion_error(e: audio_thread_priority::AudioThreadPriorityError) -> AudioError {
    AudioError::PriorityPromotion {
        source: Box::new(PromotionError(e.to_string())),
    }
}

#[derive(Debug)]
struct PromotionError(String);

impl std::fmt::Display for PromotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PromotionError {}

impl Drop for RtPriorityHandle {
    fn drop(&mut self) {
        if let Some(h) = self.inner.take() {
            let _ = audio_thread_priority::demote_current_thread_from_real_time(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_returns_handle_or_error_without_panicking() {
        match promote_current_thread() {
            Ok(_h) => {}
            Err(e) => {
                #[allow(clippy::print_stderr)]
                {
                    eprintln!("promotion unavailable in this env: {e}");
                }
            }
        }
    }
}
