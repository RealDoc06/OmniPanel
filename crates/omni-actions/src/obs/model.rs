//! OBS state kept in sync from query answers and events (no I/O).

use std::collections::HashMap;

use serde_json::{Value, json};

/// Live OBS state used for button feedback (recording, current scene...).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObsState {
    pub connected: bool,
    pub current_scene: Option<String>,
    pub recording: bool,
    pub recording_paused: bool,
    pub streaming: bool,
    pub replay_buffer: bool,
    pub virtual_cam: bool,
    pub studio_mode: bool,
    pub input_muted: HashMap<String, bool>,
    /// Linear volume multiplier 0.0 - 1.0 per input.
    pub input_volume: HashMap<String, f32>,
    /// (scene, source) -> visible
    pub source_visible: HashMap<(String, String), bool>,
}

/// A read-only request used to (re)load the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Query {
    CurrentScene,
    Record,
    Stream,
    ReplayBuffer,
    VirtualCam,
    StudioMode,
    Inputs,
    Scenes,
    InputMute(String),
    InputVolume(String),
    SceneItems(String),
}

impl Query {
    /// Queries that need no knowledge of inputs or scenes. Some fail when
    /// their feature is off (replay buffer); that just leaves the default.
    pub fn base() -> Vec<Self> {
        vec![
            Self::CurrentScene,
            Self::Record,
            Self::Stream,
            Self::ReplayBuffer,
            Self::VirtualCam,
            Self::StudioMode,
            Self::Inputs,
            Self::Scenes,
        ]
    }

    /// Request type and data.
    pub fn request(&self) -> (&'static str, Value) {
        match self {
            Self::CurrentScene => ("GetCurrentProgramScene", Value::Null),
            Self::Record => ("GetRecordStatus", Value::Null),
            Self::Stream => ("GetStreamStatus", Value::Null),
            Self::ReplayBuffer => ("GetReplayBufferStatus", Value::Null),
            Self::VirtualCam => ("GetVirtualCamStatus", Value::Null),
            Self::StudioMode => ("GetStudioModeEnabled", Value::Null),
            Self::Inputs => ("GetInputList", Value::Null),
            Self::Scenes => ("GetSceneList", Value::Null),
            Self::InputMute(input) => ("GetInputMute", json!({ "inputName": input })),
            Self::InputVolume(input) => ("GetInputVolume", json!({ "inputName": input })),
            Self::SceneItems(scene) => ("GetSceneItemList", json!({ "sceneName": scene })),
        }
    }
}

/// [`ObsState`] plus the bookkeeping needed to interpret events.
#[derive(Default)]
pub(super) struct Model {
    pub state: ObsState,
    inputs: Vec<String>,
    scenes: Vec<String>,
    /// (scene, scene item id) -> source name, to resolve item events.
    items: HashMap<(String, i64), String>,
}

/// Names under `key` of each element of an array.
fn names<'a>(list: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    list.as_array().into_iter().flatten().filter_map(move |item| item[key].as_str())
}

/// `outputState` of output events: whether the output is running now, if the event says.
fn output_active(data: &Value) -> Option<bool> {
    match data["outputState"].as_str()? {
        "OBS_WEBSOCKET_OUTPUT_STARTED"
        | "OBS_WEBSOCKET_OUTPUT_RESUMED"
        | "OBS_WEBSOCKET_OUTPUT_PAUSED"
        | "OBS_WEBSOCKET_OUTPUT_RECONNECTED" => Some(true),
        "OBS_WEBSOCKET_OUTPUT_STOPPED" => Some(false),
        // STARTING, STOPPING, RECONNECTING: keep what we know.
        _ => None,
    }
}

impl Model {
    /// Forget everything before a full reload, keeping the connection flag.
    pub fn reset(&mut self) {
        let connected = self.state.connected;
        *self = Self::default();
        self.state.connected = connected;
    }

    /// Queries that need the input and scene lists.
    pub fn follow_up(&self) -> Vec<Query> {
        let inputs = self.inputs.iter().flat_map(|i| [Query::InputMute(i.clone()), Query::InputVolume(i.clone())]);
        inputs.chain(self.scenes.iter().map(|s| Query::SceneItems(s.clone()))).collect()
    }

    /// Fold a successful answer into the state.
    pub fn apply_answer(&mut self, query: &Query, data: &Value) {
        let state = &mut self.state;
        match query {
            Query::CurrentScene => {
                state.current_scene =
                    data["sceneName"].as_str().or_else(|| data["currentProgramSceneName"].as_str()).map(str::to_owned);
            }
            Query::Record => {
                state.recording = data["outputActive"].as_bool().unwrap_or(false);
                state.recording_paused = state.recording && data["outputPaused"].as_bool().unwrap_or(false);
            }
            Query::Stream => state.streaming = data["outputActive"].as_bool().unwrap_or(false),
            Query::ReplayBuffer => state.replay_buffer = data["outputActive"].as_bool().unwrap_or(false),
            Query::VirtualCam => state.virtual_cam = data["outputActive"].as_bool().unwrap_or(false),
            Query::StudioMode => state.studio_mode = data["studioModeEnabled"].as_bool().unwrap_or(false),
            Query::Inputs => self.inputs = names(&data["inputs"], "inputName").map(str::to_owned).collect(),
            Query::Scenes => self.scenes = names(&data["scenes"], "sceneName").map(str::to_owned).collect(),
            Query::InputMute(input) => {
                state.input_muted.insert(input.clone(), data["inputMuted"].as_bool().unwrap_or(false));
            }
            Query::InputVolume(input) => {
                state.input_volume.insert(input.clone(), data["inputVolumeMul"].as_f64().unwrap_or(1.0) as f32);
            }
            Query::SceneItems(scene) => {
                for item in data["sceneItems"].as_array().into_iter().flatten() {
                    let (Some(id), Some(source)) = (item["sceneItemId"].as_i64(), item["sourceName"].as_str()) else {
                        continue;
                    };
                    self.items.insert((scene.clone(), id), source.to_owned());
                    state
                        .source_visible
                        .insert((scene.clone(), source.to_owned()), item["sceneItemEnabled"].as_bool().unwrap_or(true));
                }
            }
        }
    }

    /// Fold an event into the state. Returns `true` when the event changed
    /// the structure (scenes, inputs, items) and the state should be reloaded.
    pub fn apply_event(&mut self, kind: &str, data: &Value) -> bool {
        let state = &mut self.state;
        match kind {
            "CurrentProgramSceneChanged" => state.current_scene = data["sceneName"].as_str().map(str::to_owned),
            "RecordStateChanged" => {
                if let Some(active) = output_active(data) {
                    state.recording = active;
                    state.recording_paused = data["outputState"] == "OBS_WEBSOCKET_OUTPUT_PAUSED";
                }
            }
            "StreamStateChanged" => state.streaming = output_active(data).unwrap_or(state.streaming),
            "ReplayBufferStateChanged" => state.replay_buffer = output_active(data).unwrap_or(state.replay_buffer),
            "VirtualcamStateChanged" => state.virtual_cam = output_active(data).unwrap_or(state.virtual_cam),
            "StudioModeStateChanged" => {
                state.studio_mode = data["studioModeEnabled"].as_bool().unwrap_or(state.studio_mode)
            }
            "InputMuteStateChanged" => {
                if let (Some(input), Some(muted)) = (data["inputName"].as_str(), data["inputMuted"].as_bool()) {
                    state.input_muted.insert(input.to_owned(), muted);
                }
            }
            "InputVolumeChanged" => {
                if let (Some(input), Some(volume)) = (data["inputName"].as_str(), data["inputVolumeMul"].as_f64()) {
                    state.input_volume.insert(input.to_owned(), volume as f32);
                }
            }
            "SceneItemEnableStateChanged" => {
                let (Some(scene), Some(id), Some(enabled)) =
                    (data["sceneName"].as_str(), data["sceneItemId"].as_i64(), data["sceneItemEnabled"].as_bool())
                else {
                    return false;
                };
                match self.items.get(&(scene.to_owned(), id)) {
                    Some(source) => {
                        state.source_visible.insert((scene.to_owned(), source.clone()), enabled);
                    }
                    // An item we have never seen: reload to learn its source.
                    None => return true,
                }
            }
            "InputCreated"
            | "InputRemoved"
            | "InputNameChanged"
            | "SceneListChanged"
            | "SceneNameChanged"
            | "SceneItemCreated"
            | "SceneItemRemoved"
            | "SceneItemListReindexed"
            | "CurrentSceneCollectionChanged" => return true,
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(model: &mut Model, kind: &str, data: Value) -> bool {
        model.apply_event(kind, &data)
    }

    #[test]
    fn answers_fill_the_state() {
        let mut model = Model::default();
        model.apply_answer(&Query::CurrentScene, &json!({"sceneName": "Live"}));
        model.apply_answer(&Query::Record, &json!({"outputActive": true, "outputPaused": true}));
        model.apply_answer(&Query::Inputs, &json!({"inputs": [{"inputName": "Mic"}, {"inputName": "Desktop"}]}));
        model.apply_answer(&Query::Scenes, &json!({"scenes": [{"sceneName": "Live"}]}));
        model.apply_answer(&Query::InputMute("Mic".into()), &json!({"inputMuted": true}));
        model.apply_answer(&Query::InputVolume("Mic".into()), &json!({"inputVolumeMul": 0.5}));
        model.apply_answer(
            &Query::SceneItems("Live".into()),
            &json!({"sceneItems": [{"sceneItemId": 3, "sourceName": "Camera", "sceneItemEnabled": false}]}),
        );

        assert_eq!(model.state.current_scene.as_deref(), Some("Live"));
        assert!(model.state.recording && model.state.recording_paused);
        assert!(model.state.input_muted["Mic"]);
        assert_eq!(model.state.input_volume["Mic"], 0.5);
        assert!(!model.state.source_visible[&("Live".to_owned(), "Camera".to_owned())]);
        assert_eq!(
            model.follow_up(),
            [
                Query::InputMute("Mic".into()),
                Query::InputVolume("Mic".into()),
                Query::InputMute("Desktop".into()),
                Query::InputVolume("Desktop".into()),
                Query::SceneItems("Live".into()),
            ]
        );
    }

    #[test]
    fn output_events_track_start_pause_and_stop() {
        let mut model = Model::default();
        let record = |model: &mut Model, state: &str| event(model, "RecordStateChanged", json!({"outputState": state}));

        record(&mut model, "OBS_WEBSOCKET_OUTPUT_STARTING");
        assert!(!model.state.recording);
        record(&mut model, "OBS_WEBSOCKET_OUTPUT_STARTED");
        assert!(model.state.recording && !model.state.recording_paused);
        record(&mut model, "OBS_WEBSOCKET_OUTPUT_PAUSED");
        assert!(model.state.recording && model.state.recording_paused);
        record(&mut model, "OBS_WEBSOCKET_OUTPUT_RESUMED");
        assert!(model.state.recording && !model.state.recording_paused);
        record(&mut model, "OBS_WEBSOCKET_OUTPUT_STOPPING");
        assert!(model.state.recording);
        record(&mut model, "OBS_WEBSOCKET_OUTPUT_STOPPED");
        assert!(!model.state.recording);

        event(&mut model, "StreamStateChanged", json!({"outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}));
        event(&mut model, "VirtualcamStateChanged", json!({"outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}));
        event(&mut model, "ReplayBufferStateChanged", json!({"outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}));
        event(&mut model, "StudioModeStateChanged", json!({"studioModeEnabled": true}));
        assert!(
            model.state.streaming && model.state.virtual_cam && model.state.replay_buffer && model.state.studio_mode
        );
        event(&mut model, "StreamStateChanged", json!({"outputState": "OBS_WEBSOCKET_OUTPUT_RECONNECTING"}));
        assert!(model.state.streaming);
    }

    #[test]
    fn scene_item_events_resolve_sources_or_ask_for_a_reload() {
        let mut model = Model::default();
        model.apply_answer(
            &Query::SceneItems("Live".into()),
            &json!({"sceneItems": [{"sceneItemId": 1, "sourceName": "Camera", "sceneItemEnabled": true}]}),
        );
        let toggled =
            |id: i64, enabled: bool| json!({"sceneName": "Live", "sceneItemId": id, "sceneItemEnabled": enabled});

        assert!(!event(&mut model, "SceneItemEnableStateChanged", toggled(1, false)));
        assert!(!model.state.source_visible[&("Live".to_owned(), "Camera".to_owned())]);
        assert!(event(&mut model, "SceneItemEnableStateChanged", toggled(99, true)));
    }

    #[test]
    fn structural_events_request_a_reload_and_others_do_not() {
        let mut model = Model::default();
        for kind in
            ["InputCreated", "InputRemoved", "InputNameChanged", "SceneListChanged", "CurrentSceneCollectionChanged"]
        {
            assert!(event(&mut model, kind, Value::Null), "{kind}");
        }
        assert!(!event(&mut model, "InputMuteStateChanged", json!({"inputName": "Mic", "inputMuted": true})));
        assert!(!event(&mut model, "CurrentProgramSceneChanged", json!({"sceneName": "BRB"})));
        assert!(!event(&mut model, "SomethingNew", Value::Null));
        assert_eq!(model.state.current_scene.as_deref(), Some("BRB"));
    }

    #[test]
    fn reset_keeps_only_the_connection_flag() {
        let mut model = Model::default();
        model.state.connected = true;
        model.state.recording = true;
        model.reset();
        assert_eq!(model.state, ObsState { connected: true, ..ObsState::default() });
    }
}
