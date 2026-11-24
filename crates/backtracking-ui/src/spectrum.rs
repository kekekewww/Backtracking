use egui::{Color32, Rgba};

/// One colour of a [`Spectrum`], at `position` on its axis from the lowest band (0) to the
/// highest (1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumStop {
    pub position: f32,
    pub color: Color32,
}

/// Why a list of stops is not a [`Spectrum`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpectrumError {
    /// A spectrum has [`Spectrum::MIN_STOPS`] to [`Spectrum::MAX_STOPS`] stops; this many were given.
    StopCount(usize),
    /// The stop at this index, in the order given, is not at a finite position in 0..=1.
    Position(usize),
}

/// The colours a waveform takes from its frequency content: a gradient of 2 to 8 stops laid
/// on an axis from the lowest frequency band to the highest.
///
/// Bands sit evenly along the axis, the lowest at 0 and the highest at 1, and each takes the
/// gradient's colour there. A column whose energy lies in one band is exactly that band's
/// colour. Any other column blends the band colours in proportion to their energy, in linear
/// light: mixing by energy is mixing light, and linear light is where light adds, so two bands
/// at equal energy look like both lamps lit at half power, without the darkening a blend of
/// sRGB values gives. (Oklab would keep mixtures more even in perceived lightness, but a
/// mixture is meant to look like its parts added up.) Stops between two band positions shade
/// those bands' colours; with six bands and more than six stops, a stop between band
/// positions can only show through its neighbours.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    stops: Vec<SpectrumStop>,
}

impl Spectrum {
    pub const MIN_STOPS: usize = 2;
    pub const MAX_STOPS: usize = 8;
    /// The default lowest-band colour.
    pub const DEFAULT_LOW: Color32 = Color32::from_rgb(232, 48, 44);
    /// The default highest-band colour.
    pub const DEFAULT_HIGH: Color32 = Color32::from_rgb(44, 104, 236);

    /// Sorts the stops by position, keeping the given order of stops at the same position.
    /// Two stops at one position make a hard edge there: the later one's colour starts at it.
    pub fn new(stops: impl IntoIterator<Item = SpectrumStop>) -> Result<Self, SpectrumError> {
        let mut stops: Vec<_> = stops.into_iter().take(Self::MAX_STOPS + 1).collect();
        if !(Self::MIN_STOPS..=Self::MAX_STOPS).contains(&stops.len()) {
            return Err(SpectrumError::StopCount(stops.len()));
        }
        if let Some(index) = stops
            .iter()
            .position(|stop| !(0.0..=1.0).contains(&stop.position))
        {
            return Err(SpectrumError::Position(index));
        }
        stops.sort_by(|a, b| a.position.total_cmp(&b.position));
        Ok(Self { stops })
    }

    /// A two-colour spectrum from `low` at 0 to `high` at 1.
    pub fn two(low: Color32, high: Color32) -> Self {
        Self {
            stops: vec![
                SpectrumStop {
                    position: 0.0,
                    color: low,
                },
                SpectrumStop {
                    position: 1.0,
                    color: high,
                },
            ],
        }
    }

    /// The stops, sorted by position.
    pub fn stops(&self) -> &[SpectrumStop] {
        &self.stops
    }

    /// The gradient's colour at `position`, interpolated in linear light. Exactly a stop's
    /// colour at its position; the outer stops' colours before the first and after the last.
    pub fn at(&self, position: f32) -> Color32 {
        let stops = &self.stops;
        let next = stops.partition_point(|stop| stop.position <= position);
        if next == 0 {
            return stops[0].color;
        }
        let stop = stops[next - 1];
        let Some(&after) = stops.get(next) else {
            return stop.color;
        };
        if stop.position == position {
            return stop.color;
        }
        let t = (position - stop.position) / (after.position - stop.position);
        let (from, to) = (Rgba::from(stop.color), Rgba::from(after.color));
        (from * (1.0 - t) + to * t).into()
    }

    /// The colours of `bands` evenly spaced bands, for colouring many columns.
    pub fn palette(&self, bands: usize) -> SpectrumPalette {
        let colors = (0..bands)
            .map(|band| {
                let position = if bands > 1 {
                    band as f32 / (bands - 1) as f32
                } else {
                    0.0
                };
                let color = self.at(position);
                (color, Rgba::from(color))
            })
            .collect();
        SpectrumPalette { colors }
    }

    /// The colour of one column from its band energies, lowest band first; see
    /// [`SpectrumPalette::color`]. When colouring many columns, make the palette once.
    pub fn color(&self, energies: &[f32]) -> Option<Color32> {
        self.palette(energies.len()).color(energies)
    }
}

impl Default for Spectrum {
    /// Red for the lowest band to blue for the highest.
    fn default() -> Self {
        Self::two(Self::DEFAULT_LOW, Self::DEFAULT_HIGH)
    }
}

/// A [`Spectrum`]'s colour for each of a fixed number of bands.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPalette {
    colors: Vec<(Color32, Rgba)>,
}

impl SpectrumPalette {
    /// The colour of one band, or `None` past the last.
    pub fn band(&self, band: usize) -> Option<Color32> {
        self.colors.get(band).map(|&(color, _)| color)
    }

    /// Blends the band colours in proportion to `energies`, in linear light. Energy in one
    /// band only gives exactly that band's colour. Negative and non-finite energies count as
    /// none, and bands past the palette's are ignored. `None` when no band has energy, which
    /// is how no data reads.
    pub fn color(&self, energies: &[f32]) -> Option<Color32> {
        let weight = |energy: f32| {
            if energy.is_finite() && energy > 0.0 {
                f64::from(energy)
            } else {
                0.0
            }
        };
        let mut total = 0.0;
        let mut only = None;
        let mut lit = 0;
        for (band, &energy) in energies.iter().take(self.colors.len()).enumerate() {
            let energy = weight(energy);
            if energy > 0.0 {
                total += energy;
                only = Some(band);
                lit += 1;
            }
        }
        if total <= 0.0 {
            return None;
        }
        if lit == 1 {
            return only.map(|band| self.colors[band].0);
        }
        let mut sum = Rgba::TRANSPARENT;
        for (&(_, color), &energy) in self.colors.iter().zip(energies) {
            sum = sum + color * (weight(energy) / total) as f32;
        }
        Some(sum.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(position: f32, color: Color32) -> SpectrumStop {
        SpectrumStop { position, color }
    }

    const GREEN: Color32 = Color32::from_rgb(40, 200, 90);

    #[test]
    fn one_band_alone_is_exactly_the_colour_at_its_position_for_two_and_eight_stops() {
        let two = Spectrum::default();
        let eight = Spectrum::new((0..8).map(|index| {
            let position = index as f32 / 7.0;
            stop(
                position,
                Color32::from_rgb(30 * index as u8, 200 - 25 * index as u8, 17 + index as u8),
            )
        }))
        .unwrap();
        for spectrum in [&two, &eight] {
            let palette = spectrum.palette(6);
            for band in 0..6 {
                let mut energies = [0.0; 6];
                energies[band] = 0.37;
                let expected = spectrum.at(band as f32 / 5.0);
                assert_eq!(spectrum.color(&energies), Some(expected));
                assert_eq!(palette.color(&energies), Some(expected));
                assert_eq!(palette.band(band), Some(expected));
            }
        }
        assert_eq!(
            two.color(&[2.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            Some(Spectrum::DEFAULT_LOW)
        );
        assert_eq!(
            two.color(&[0.0, 0.0, 0.0, 0.0, 0.0, 1e-9]),
            Some(Spectrum::DEFAULT_HIGH)
        );
        // Eight stops at the band positions' neighbours: a stop at 1/7 shades band 1 (at 0.2)
        // between the second and third stops.
        assert_eq!(eight.at(0.0), eight.stops()[0].color);
        assert_eq!(eight.at(1.0), eight.stops()[7].color);
        assert_ne!(eight.at(0.2), eight.stops()[1].color);
    }

    #[test]
    fn mixtures_blend_in_linear_light_by_energy() {
        let spectrum = Spectrum::default();
        let even = spectrum.color(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0]).unwrap();
        let expected = Color32::from(
            Rgba::from(Spectrum::DEFAULT_LOW) * 0.5 + Rgba::from(Spectrum::DEFAULT_HIGH) * 0.5,
        );
        assert_eq!(even, expected);
        // Linear light keeps an even mixture of red and blue bright, unlike a blend of sRGB values.
        assert!(
            even.r()
                > (Spectrum::DEFAULT_LOW.r() as u16 + Spectrum::DEFAULT_HIGH.r() as u16) as u8 / 2
        );
        // Scaling every energy changes nothing; moving energy up moves the colour toward blue.
        assert_eq!(spectrum.color(&[3.0, 0.0, 0.0, 0.0, 0.0, 3.0]), Some(even));
        let lower = spectrum.color(&[3.0, 1.0, 0.0, 0.0, 0.0, 1.0]).unwrap();
        let higher = spectrum.color(&[1.0, 0.0, 0.0, 0.0, 1.0, 3.0]).unwrap();
        assert!(lower.r() > even.r() && even.r() > higher.r());
        assert!(lower.b() < even.b() && even.b() < higher.b());
        // A gradient through green blends band colours, not positions: low and high at equal
        // energy are not the green in the middle.
        let three = Spectrum::new([
            stop(0.0, Spectrum::DEFAULT_LOW),
            stop(0.5, GREEN),
            stop(1.0, Spectrum::DEFAULT_HIGH),
        ])
        .unwrap();
        assert_eq!(three.color(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0]), Some(even));
        assert_eq!(three.at(0.5), GREEN);
    }

    #[test]
    fn no_energy_is_no_colour() {
        let spectrum = Spectrum::default();
        assert_eq!(spectrum.color(&[]), None);
        assert_eq!(spectrum.color(&[0.0; 6]), None);
        assert_eq!(
            spectrum.color(&[-1.0, f32::NAN, 0.0, f32::NEG_INFINITY, -0.0, 0.0]),
            None
        );
        // Bad values beside good ones count as no energy.
        assert_eq!(
            spectrum.color(&[1.0, f32::NAN, -4.0, f32::INFINITY, 0.0, 0.0]),
            Some(Spectrum::DEFAULT_LOW)
        );
        let huge = spectrum.color(&[f32::MAX, 0.0, 0.0, 0.0, 0.0, f32::MAX]);
        assert_eq!(huge, spectrum.color(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn stops_are_validated_and_sorted() {
        let red = Spectrum::DEFAULT_LOW;
        let blue = Spectrum::DEFAULT_HIGH;
        assert_eq!(
            Spectrum::new([stop(0.0, red)]),
            Err(SpectrumError::StopCount(1))
        );
        assert_eq!(Spectrum::new([]), Err(SpectrumError::StopCount(0)));
        assert_eq!(
            Spectrum::new([stop(0.5, red); 9]),
            Err(SpectrumError::StopCount(9))
        );
        assert_eq!(
            Spectrum::new([stop(0.5, red); 12]),
            Err(SpectrumError::StopCount(9))
        );
        for position in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert_eq!(
                Spectrum::new([stop(0.0, red), stop(position, blue)]),
                Err(SpectrumError::Position(1))
            );
        }
        let sorted = Spectrum::new([stop(1.0, blue), stop(0.25, GREEN), stop(0.0, red)]).unwrap();
        let positions: Vec<_> = sorted.stops().iter().map(|stop| stop.position).collect();
        assert_eq!(positions, [0.0, 0.25, 1.0]);
        assert_eq!(sorted.at(0.25), GREEN);
        assert_eq!(
            Spectrum::new([stop(0.0, red), stop(1.0, blue)]).unwrap(),
            Spectrum::default()
        );
        // Stops need not reach the ends, and a shared position is a hard edge.
        let inner = Spectrum::new([stop(0.3, red), stop(0.6, GREEN), stop(0.6, blue)]).unwrap();
        assert_eq!(inner.at(0.0), red);
        assert_eq!(inner.at(0.3), red);
        assert_eq!(inner.at(0.6), blue);
        assert_eq!(inner.at(1.0), blue);
        let just_below = inner.at(0.599);
        assert!(just_below.g() > just_below.b(), "{just_below:?}");
    }

    #[test]
    fn other_band_counts_spread_evenly_along_the_axis() {
        let spectrum = Spectrum::default();
        assert_eq!(spectrum.palette(1).band(0), Some(Spectrum::DEFAULT_LOW));
        let three = spectrum.palette(3);
        assert_eq!(three.band(0), Some(Spectrum::DEFAULT_LOW));
        assert_eq!(three.band(1), Some(spectrum.at(0.5)));
        assert_eq!(three.band(2), Some(Spectrum::DEFAULT_HIGH));
        assert_eq!(three.band(3), None);
        // Extra energies past the palette are ignored.
        assert_eq!(
            three.color(&[0.0, 0.0, 1.0, 5.0]),
            Some(Spectrum::DEFAULT_HIGH)
        );
    }
}
