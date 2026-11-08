//! `config.show`: a lane's project config after every file is merged and the
//! templates are expanded, for `devlanes debug config`.

use std::sync::Arc;

use devlanes_proto::{Context, Method, RpcError};
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    registry::internal,
    target::{LaneOrFile, Policy},
};

impl Inner {
    /// `config.show`: `{project, lane, files, config}`, with `files` in merge order.
    /// Expressions stay as `${{ ... }}` text, so `.env` and client values never
    /// reach the answer; literal values in the files do. In a project nobody
    /// started it shows the project file with `lane: null`.
    pub(crate) async fn config_show(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let (name, reference, config) = match self
            .lane_or_file(context, Policy::of(Method::ConfigShow))
            .await?
        {
            LaneOrFile::Lane((project, lane)) => {
                self.fresh_config(&project, &lane).await;
                let st = lane.st();
                let reference = st.reference(&project.name());
                let config = st.config.require(&reference)?;
                (project.name(), Some(reference), config)
            }
            LaneOrFile::File { config, .. } => (config.project.clone(), None, config),
        };
        let files: Vec<&str> = config.sources.files().collect();
        let value = serde_json::to_value(config.as_ref())
            .map_err(|error| internal("config_show", error))?;
        Ok(json!({
            "project": name,
            "lane": reference,
            "files": files,
            "config": value,
        }))
    }
}
