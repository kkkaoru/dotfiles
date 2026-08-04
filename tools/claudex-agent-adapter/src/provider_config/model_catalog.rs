use super::{ModelCatalog, push_nonempty};

impl ModelCatalog {
    /// Exact model ids for Claude Code's `/model` picker (defaults, workers, advisors).
    pub fn selectable_models(&self) -> Vec<String> {
        let mut models = self.exact.clone();
        for worker in self
            .workers
            .iter()
            .chain(self.search_workers.iter())
            .chain(self.auxiliary_workers.iter())
        {
            push_nonempty(&mut models, &worker.model);
        }
        models.sort();
        models.dedup();
        models
    }
}
