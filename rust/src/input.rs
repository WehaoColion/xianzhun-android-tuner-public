/// Input measurements from PCM, independent of pitch or string recognition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputFrame {
    /// RMS of the AC component, normalized by the PCM 16-bit scale.
    pub rms: f32,
    /// Peak of the AC component, normalized by the PCM 16-bit scale.
    /// A heavily biased signal can have an AC peak greater than one.
    pub peak: f32,
    /// True when every sample has exactly the same value, including DC-only PCM.
    pub digital_silence: bool,
}

impl InputFrame {
    /// Empty reads do not establish that a frame was received.
    pub fn from_pcm(samples: &[i16]) -> Option<Self> {
        let &first = samples.first()?;
        let mut sum = 0.0_f64;
        let mut digital_silence = true;
        for &sample in samples {
            sum += f64::from(sample);
            digital_silence &= sample == first;
        }
        let mean = sum / samples.len() as f64;
        let mut energy = 0.0_f64;
        let mut peak = 0.0_f64;
        for &sample in samples {
            let centered = (f64::from(sample) - mean) / 32_768.0;
            energy += centered * centered;
            peak = peak.max(centered.abs());
        }
        Some(Self {
            rms: (energy / samples.len() as f64).sqrt() as f32,
            peak: peak as f32,
            digital_silence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::InputFrame;

    #[test]
    fn empty_read_has_no_input_measurement() {
        assert_eq!(InputFrame::from_pcm(&[]), None);
    }

    #[test]
    fn zero_or_constant_dc_is_digital_silence() {
        for value in [0, 1, -1, i16::MIN, i16::MAX] {
            let input = InputFrame::from_pcm(&[value; 32]).unwrap();
            assert!(input.digital_silence);
            assert_eq!(input.rms, 0.0);
            assert_eq!(input.peak, 0.0);
        }
    }

    #[test]
    fn dc_bias_does_not_hide_or_inflate_ac_input() {
        let centered = InputFrame::from_pcm(&[-8192, 8192, -8192, 8192]).unwrap();
        let biased = InputFrame::from_pcm(&[8192, 24576, 8192, 24576]).unwrap();
        assert_eq!(centered, biased);
        assert!(!biased.digital_silence);
        assert_eq!(biased.rms, 0.25);
        assert_eq!(biased.peak, 0.25);
    }

    #[test]
    fn full_signed_range_is_finite_without_overflow() {
        let input = InputFrame::from_pcm(&[i16::MIN, i16::MAX]).unwrap();
        let full_range_amplitude = 65535.0 / 65536.0;
        assert!(!input.digital_silence);
        assert!((input.rms - full_range_amplitude).abs() < 1.0e-6);
        assert!((input.peak - full_range_amplitude).abs() < 1.0e-6);
        assert!(input.rms.is_finite() && input.peak.is_finite());
    }

    #[test]
    fn weak_variation_still_proves_pcm_input_without_a_valid_pitch() {
        let input = InputFrame::from_pcm(&[-1, 1, -1, 1]).unwrap();
        assert!(!input.digital_silence);
        assert_eq!(input.rms, 1.0 / 32768.0);
        assert_eq!(input.peak, 1.0 / 32768.0);
    }

    #[test]
    fn tiny_variation_near_signed_limit_survives_dc_removal() {
        let input = InputFrame::from_pcm(&[i16::MAX - 1, i16::MAX]).unwrap();
        assert!(!input.digital_silence);
        assert_eq!(input.rms, 0.5 / 32768.0);
        assert_eq!(input.peak, 0.5 / 32768.0);
    }

    #[test]
    fn single_sample_is_received_but_has_no_ac_component() {
        let input = InputFrame::from_pcm(&[i16::MIN]).unwrap();
        assert!(input.digital_silence);
        assert_eq!(input.rms, 0.0);
        assert_eq!(input.peak, 0.0);
    }
}
