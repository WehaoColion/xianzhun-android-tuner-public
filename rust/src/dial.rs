//! Shared normalized geometry for the brass VectorDrawable and live needle.
pub(crate) const WIDTH: f32 = 400.0;
pub(crate) const HEIGHT: f32 = 225.0;
pub(crate) const PIVOT_X: f32 = 200.0;
pub(crate) const PIVOT_Y: f32 = 205.0;
pub(crate) const NEEDLE_RADIUS: f32 = 148.0;
pub(crate) const LABEL_RADIUS: f32 = 180.0;
pub(crate) const LABEL_CENTS: [f32; 5] = [-50.0, -25.0, 0.0, 25.0, 50.0];

pub(crate) fn angle(cents: f32) -> f32 {
    cents.clamp(-50.0, 50.0) * 1.5
}

pub(crate) fn label_center(cents: f32) -> (f32, f32) {
    let theta = angle(cents).to_radians();
    (PIVOT_X + LABEL_RADIUS * theta.sin(), PIVOT_Y - LABEL_RADIUS * theta.cos())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pitch_labels_and_needle_share_angular_mapping() {
        assert_eq!(angle(-220.0), -75.0);
        assert_eq!(angle(220.0), 75.0);
        assert_eq!(label_center(0.0), (PIVOT_X, PIVOT_Y - LABEL_RADIUS));
        for cents in LABEL_CENTS {
            let (x, y) = label_center(cents);
            let theta = (x - PIVOT_X).atan2(PIVOT_Y - y).to_degrees();
            assert!((theta - angle(cents)).abs() < 0.001);
            assert!(x > 20.0 && x < WIDTH - 20.0 && y > 0.0 && y < HEIGHT);
            assert!(NEEDLE_RADIUS < LABEL_RADIUS);
        }
    }
}
