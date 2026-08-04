use super::WorkerRoute;

impl WorkerRoute {
    pub fn new(
        agent: impl Into<String>,
        model: impl Into<String>,
        effort: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            model: model.into(),
            effort: effort.into(),
            usage_provider: None,
        }
    }

    pub(super) fn with_usage_provider(mut self, usage_provider: Option<String>) -> Self {
        self.usage_provider = usage_provider;
        self
    }
}
