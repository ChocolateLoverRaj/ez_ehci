pub struct EhciRunner {}

impl EhciRunner {
    /// Guaranteed to not block, acquire a mutex, or allocate.
    pub fn handle_irq(&self) {}
}
