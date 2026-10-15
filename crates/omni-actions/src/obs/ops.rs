//! `obs` actions and the editor inventory.

use futures_util::future::{try_join, try_join_all};
use omni_core::{ActionOutcome, ObsInventory, ObsOp, ObsSceneItems};
use serde_json::{Value, json};

use super::ObsClient;
use crate::{ActionError, Result};

impl ObsClient {
    pub(crate) async fn run(&self, op: &ObsOp) -> Result<ActionOutcome> {
        match op {
            ObsOp::SetScene { scene } => self.request("SetCurrentProgramScene", json!({ "sceneName": scene })).await?,
            ObsOp::ToggleRecord => self.request("ToggleRecord", Value::Null).await?,
            ObsOp::StartRecord => self.request("StartRecord", Value::Null).await?,
            ObsOp::StopRecord => self.request("StopRecord", Value::Null).await?,
            // Pauses a running recording and resumes a paused one.
            ObsOp::PauseRecord => self.request("ToggleRecordPause", Value::Null).await?,
            ObsOp::ToggleStream => self.request("ToggleStream", Value::Null).await?,
            ObsOp::StartStream => self.request("StartStream", Value::Null).await?,
            ObsOp::StopStream => self.request("StopStream", Value::Null).await?,
            ObsOp::ToggleReplayBuffer => self.request("ToggleReplayBuffer", Value::Null).await?,
            ObsOp::SaveReplay => self.request("SaveReplayBuffer", Value::Null).await?,
            ObsOp::ToggleVirtualCam => self.request("ToggleVirtualCam", Value::Null).await?,
            ObsOp::ToggleMute { input } => self.request("ToggleInputMute", json!({ "inputName": input })).await?,
            ObsOp::ToggleSource { scene, source } => self.toggle_source(scene, source).await?,
            ObsOp::ToggleStudioMode => {
                let current = self.request("GetStudioModeEnabled", Value::Null).await?;
                let enabled = current["studioModeEnabled"].as_bool().unwrap_or(false);
                self.request("SetStudioModeEnabled", json!({ "studioModeEnabled": !enabled })).await?
            }
        };
        Ok(ActionOutcome { message: None })
    }

    /// Show / hide a source inside a scene.
    async fn toggle_source(&self, scene: &str, source: &str) -> Result<Value> {
        let found = self.request("GetSceneItemId", json!({ "sceneName": scene, "sourceName": source })).await?;
        let id = found["sceneItemId"]
            .as_i64()
            .ok_or_else(|| ActionError::Failed(format!("OBS has no item \"{source}\" in \"{scene}\"")))?;
        let current = self.request("GetSceneItemEnabled", json!({ "sceneName": scene, "sceneItemId": id })).await?;
        let enabled = current["sceneItemEnabled"].as_bool().unwrap_or(false);
        self.request(
            "SetSceneItemEnabled",
            json!({ "sceneName": scene, "sceneItemId": id, "sceneItemEnabled": !enabled }),
        )
        .await
    }

    /// Scenes, inputs and scene items for the editor pickers.
    pub async fn inventory(&self) -> Result<ObsInventory> {
        let (scenes, inputs) =
            try_join(self.request("GetSceneList", Value::Null), self.request("GetInputList", Value::Null)).await?;
        let scenes = names(&scenes["scenes"], "sceneName");
        let inputs = names(&inputs["inputs"], "inputName");
        let scene_items = try_join_all(scenes.iter().map(|scene| async move {
            let items = self.request("GetSceneItemList", json!({ "sceneName": scene })).await?;
            let mut sources = names(&items["sceneItems"], "sourceName");
            let mut seen = std::collections::HashSet::new();
            sources.retain(|source| seen.insert(source.clone()));
            Ok::<_, ActionError>(ObsSceneItems { scene: scene.clone(), sources })
        }))
        .await?;
        Ok(ObsInventory { scenes, inputs, scene_items })
    }
}

fn names(list: &Value, key: &str) -> Vec<String> {
    list.as_array().into_iter().flatten().filter_map(|item| item[key].as_str()).map(str::to_owned).collect()
}
