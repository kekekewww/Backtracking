use std::{ops::Range, sync::Arc};

/// The complete Channels group in lane-relative order. Copies share its labels and weights.
#[derive(Clone, Debug, PartialEq)]
pub struct PairGroup {
    pub(crate) first: usize,
    pub(crate) labels: Arc<[String]>,
    pub(crate) weights: Arc<[[f32; 2]]>,
}

impl PairGroup {
    pub fn new(first: usize, labels: &[String]) -> Option<Self> {
        if labels.is_empty() || first.checked_add(labels.len())? > 64 {
            return None;
        }
        let names: Vec<_> = labels.iter().map(String::as_str).collect();
        Some(Self {
            first,
            labels: labels.into(),
            weights: crate::spatial::stereo_weights(&names).into(),
        })
    }

    pub(crate) fn channels(&self) -> Range<usize> {
        self.first..self.first + self.labels.len()
    }

    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.first == other.first
            && (Arc::ptr_eq(&self.labels, &other.labels) || self.labels == other.labels)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairSide {
    Left,
    Right,
}
