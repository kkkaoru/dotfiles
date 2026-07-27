use super::RoutedBackends;

impl RoutedBackends {
    pub(crate) fn max_concurrency_for_model(&self, model: &str) -> Option<usize> {
        if let Some(route) = self.find(model) {
            return route.template.max_concurrency;
        }
        self.prefix_template(model)
            .and_then(|template| template.max_concurrency)
    }

    pub(crate) fn configured_concurrency_limits(&self) -> Vec<(String, usize)> {
        self.configured
            .iter()
            .filter_map(|route| {
                route
                    .template
                    .max_concurrency
                    .map(|limit| (route.model.clone(), limit))
            })
            .collect()
    }
}
