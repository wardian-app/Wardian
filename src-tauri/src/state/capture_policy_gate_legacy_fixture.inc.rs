// Fixture-only helpers included by the owning module during tests.

impl CapturePolicyGate {
    /// Optimistic recent-header admission must stand aside for a registered
    /// lifecycle boundary without waiting for older archive IO.
    pub(crate) fn background_admission_open(&self) -> bool {
        self.lifecycle_boundaries.load(Ordering::SeqCst) == 0
    }
}
