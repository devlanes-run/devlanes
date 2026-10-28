//! `config.show`: a lane's project config after every file is merged and the
//! templates are expanded, for `devlanes debug config`.

use std::sync::Arc;

use devlanes_proto::{Context, RpcError};
use serde_json::{Value, json};

use crate::{engine::Inner, registry::internal};

impl Inner {
    /// `config.show`: `{project, lane, files, config}`, with `files` in merge order.
    /// Expressions stay as `${{ ... }}` text, so `.env` and client values never
    /// reach the answer; literal values in the files do.
    pub(crate) async fn config_show(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let (project, lane) = self.target_lane(context).await?;
        self.fresh_config(&project, &lane).await;
        let name = project.name();
        let (reference, config) = {
            let st = lane.st();
            let reference = st.reference(&name);
            let config = st.config.require(&reference)?;
            (reference, config)
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
