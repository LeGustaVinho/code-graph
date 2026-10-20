//! Unity engine knowledge: which methods the engine / editor invokes by name, on which base
//! types, and the part of Unity's own class hierarchy that project code derives from.
//!
//! Extraction only marks *candidates* (a method named like a message, in a class with
//! bases); the global pass in `codegraph_semantic::unity` confirms them by walking the real
//! inheritance chain (project base classes included) up to one of these bases.

const MONO_BEHAVIOUR: &[&str] = &[
    "Awake",
    "Start",
    "Update",
    "FixedUpdate",
    "LateUpdate",
    "OnEnable",
    "OnDisable",
    "OnDestroy",
    "OnValidate",
    "Reset",
    "OnGUI",
    "OnApplicationPause",
    "OnApplicationFocus",
    "OnApplicationQuit",
    "OnTriggerEnter",
    "OnTriggerExit",
    "OnTriggerStay",
    "OnTriggerEnter2D",
    "OnTriggerExit2D",
    "OnTriggerStay2D",
    "OnCollisionEnter",
    "OnCollisionExit",
    "OnCollisionStay",
    "OnCollisionEnter2D",
    "OnCollisionExit2D",
    "OnCollisionStay2D",
    "OnControllerColliderHit",
    "OnMouseDown",
    "OnMouseUp",
    "OnMouseUpAsButton",
    "OnMouseEnter",
    "OnMouseExit",
    "OnMouseOver",
    "OnMouseDrag",
    "OnBecameVisible",
    "OnBecameInvisible",
    "OnDrawGizmos",
    "OnDrawGizmosSelected",
    "OnRenderObject",
    "OnPreRender",
    "OnPostRender",
    "OnPreCull",
    "OnWillRenderObject",
    "OnRenderImage",
    "OnAnimatorMove",
    "OnAnimatorIK",
    "OnParticleCollision",
    "OnParticleTrigger",
    "OnParticleSystemStopped",
    "OnParticleUpdateJobScheduled",
    "OnTransformParentChanged",
    "OnTransformChildrenChanged",
    "OnRectTransformDimensionsChange",
    "OnCanvasGroupChanged",
    "OnCanvasHierarchyChanged",
    "OnDidApplyAnimationProperties",
    "OnBeforeTransformParentChanged",
    "OnJointBreak",
    "OnJointBreak2D",
    "OnAudioFilterRead",
    "OnServerInitialized",
    "OnConnectedToServer",
    "OnDisconnectedFromServer",
    "OnPlayerConnected",
    "OnPlayerDisconnected",
    "OnFailedToConnect",
    "OnNetworkInstantiate",
    "OnSerializeNetworkView",
];

const SCRIPTABLE_OBJECT: &[&str] = &["Awake", "OnEnable", "OnDisable", "OnDestroy", "OnValidate", "Reset"];

const EDITOR: &[&str] = &[
    "OnInspectorGUI",
    "OnSceneGUI",
    "OnEnable",
    "OnDisable",
    "OnDestroy",
    "Awake",
    "Reset",
    "OnValidate",
    "CreateInspectorGUI",
    "OnPreviewGUI",
    "OnInteractivePreviewGUI",
    "OnPreviewSettings",
    "HasPreviewGUI",
    "RenderStaticPreview",
    "GetPreviewTitle",
    "OnHeaderGUI",
    "RequiresConstantRepaint",
    "HasFrameBounds",
    "OnGetFrameBounds",
    "UseDefaultMargins",
    "GetInfoString",
    "DrawPreview",
    "ShouldHideOpenButton",
];

const EDITOR_WINDOW: &[&str] = &[
    "OnGUI",
    "CreateGUI",
    "Update",
    "OnEnable",
    "OnDisable",
    "OnDestroy",
    "Awake",
    "OnFocus",
    "OnLostFocus",
    "OnSelectionChange",
    "OnHierarchyChange",
    "OnProjectChange",
    "OnInspectorUpdate",
    "OnAddedAsTab",
    "OnBeforeRemovedAsTab",
    "OnBecameVisible",
    "OnBecameInvisible",
    "ShowButton",
    "OnDidOpenScene",
    "OnValidate",
    "ModifierKeysChanged",
];

const PROPERTY_DRAWER: &[&str] = &["OnGUI", "GetPropertyHeight", "CreatePropertyGUI", "CanCacheInspectorGUI"];
const DECORATOR_DRAWER: &[&str] = &["OnGUI", "GetHeight", "CreatePropertyGUI", "CanCacheInspectorGUI"];

const ASSET_POSTPROCESSOR: &[&str] = &[
    "OnPreprocessAsset",
    "OnPreprocessTexture",
    "OnPostprocessTexture",
    "OnPreprocessModel",
    "OnPostprocessModel",
    "OnPreprocessAudio",
    "OnPostprocessAudio",
    "OnPreprocessAnimation",
    "OnPostprocessAnimation",
    "OnPostprocessSprites",
    "OnPostprocessMaterial",
    "OnPostprocessPrefab",
    "OnPostprocessGameObjectWithUserProperties",
    "OnPostprocessGameObjectWithAnimatedUserProperties",
    "OnAssignMaterialModel",
    "OnPostprocessMeshHierarchy",
    "OnPostprocessAllAssets",
    "OnPreprocessSpeedTree",
    "OnPostprocessSpeedTree",
    "OnPreprocessCameraDescription",
    "OnPreprocessLightDescription",
    "OnPreprocessMaterialDescription",
    "OnPostprocessCubemap",
    "OnPostprocessTexture2DArray",
    "OnPostprocessTexture3D",
    "GetPostprocessOrder",
    "GetVersion",
];

const STATE_MACHINE_BEHAVIOUR: &[&str] =
    &["OnStateEnter", "OnStateExit", "OnStateUpdate", "OnStateMove", "OnStateIK", "OnStateMachineEnter", "OnStateMachineExit"];

const SCRIPTABLE_WIZARD: &[&str] = &["OnWizardCreate", "OnWizardUpdate", "OnWizardOtherButton", "OnDrawGizmos"];

const NETWORK_BEHAVIOUR: &[&str] = &[
    // Netcode for GameObjects
    "OnNetworkSpawn",
    "OnNetworkDespawn",
    "OnGainedOwnership",
    "OnLostOwnership",
    "OnNetworkObjectParentChanged",
    // Mirror
    "OnStartServer",
    "OnStopServer",
    "OnStartClient",
    "OnStopClient",
    "OnStartLocalPlayer",
    "OnStopLocalPlayer",
    "OnStartAuthority",
    "OnStopAuthority",
    "OnSerialize",
    "OnDeserialize",
];

/// Interfaces the engine calls through (UGUI events, serialization, build pipeline).
const INTERFACE_MESSAGES: &[(&str, &[&str])] = &[
    ("ISerializationCallbackReceiver", &["OnBeforeSerialize", "OnAfterDeserialize"]),
    ("IPointerClickHandler", &["OnPointerClick"]),
    ("IPointerDownHandler", &["OnPointerDown"]),
    ("IPointerUpHandler", &["OnPointerUp"]),
    ("IPointerEnterHandler", &["OnPointerEnter"]),
    ("IPointerExitHandler", &["OnPointerExit"]),
    ("IPointerMoveHandler", &["OnPointerMove"]),
    ("IBeginDragHandler", &["OnBeginDrag"]),
    ("IDragHandler", &["OnDrag"]),
    ("IEndDragHandler", &["OnEndDrag"]),
    ("IDropHandler", &["OnDrop"]),
    ("IScrollHandler", &["OnScroll"]),
    ("ISelectHandler", &["OnSelect"]),
    ("IDeselectHandler", &["OnDeselect"]),
    ("IMoveHandler", &["OnMove"]),
    ("ISubmitHandler", &["OnSubmit"]),
    ("ICancelHandler", &["OnCancel"]),
    ("IUpdateSelectedHandler", &["OnUpdateSelected"]),
    ("IInitializePotentialDragHandler", &["OnInitializePotentialDrag"]),
    ("IPreprocessBuildWithReport", &["OnPreprocessBuild"]),
    ("IPostprocessBuildWithReport", &["OnPostprocessBuild"]),
    ("IPreprocessBuild", &["OnPreprocessBuild"]),
    ("IPostprocessBuild", &["OnPostprocessBuild"]),
    ("IProcessSceneWithReport", &["OnProcessScene"]),
    ("IProcessScene", &["OnProcessScene"]),
    ("IPreprocessShaders", &["OnProcessShader"]),
    ("IPreprocessComputeShaders", &["OnProcessComputeShader"]),
    ("IActiveBuildTargetChanged", &["OnActiveBuildTargetChanged"]),
    ("IPostBuildPlayerScriptDLLs", &["OnPostBuildPlayerScriptDLLs"]),
    ("IFilterBuildAssemblies", &["OnFilterAssemblies"]),
];

/// Engine-invoked methods declared by a Unity base type / interface.
pub fn messages_of(base: &str) -> &'static [&'static str] {
    match base {
        "MonoBehaviour" => MONO_BEHAVIOUR,
        "ScriptableObject" => SCRIPTABLE_OBJECT,
        "Editor" => EDITOR,
        "EditorWindow" => EDITOR_WINDOW,
        "PropertyDrawer" => PROPERTY_DRAWER,
        "DecoratorDrawer" => DECORATOR_DRAWER,
        "AssetPostprocessor" => ASSET_POSTPROCESSOR,
        "StateMachineBehaviour" => STATE_MACHINE_BEHAVIOUR,
        "ScriptableWizard" => SCRIPTABLE_WIZARD,
        "NetworkBehaviour" => NETWORK_BEHAVIOUR,
        _ => INTERFACE_MESSAGES.iter().find(|(i, _)| *i == base).map(|(_, m)| *m).unwrap_or(&[]),
    }
}

/// Unity / package types project code derives from, mapped to their own Unity base(s):
/// a class deriving from `Button` is a MonoBehaviour even though `Button` is external.
pub fn implied_bases(base: &str) -> &'static [&'static str] {
    match base {
        "Behaviour"
        | "UIBehaviour"
        | "Selectable"
        | "Button"
        | "Toggle"
        | "Slider"
        | "Scrollbar"
        | "ScrollRect"
        | "Dropdown"
        | "TMP_Dropdown"
        | "InputField"
        | "TMP_InputField"
        | "Graphic"
        | "MaskableGraphic"
        | "Image"
        | "RawImage"
        | "Text"
        | "TMP_Text"
        | "TextMeshProUGUI"
        | "TextMeshPro"
        | "LayoutGroup"
        | "HorizontalLayoutGroup"
        | "VerticalLayoutGroup"
        | "GridLayoutGroup"
        | "HorizontalOrVerticalLayoutGroup"
        | "ContentSizeFitter"
        | "AspectRatioFitter"
        | "BaseMeshEffect"
        | "Shadow"
        | "Outline"
        | "EventTrigger"
        | "BaseRaycaster"
        | "PhysicsRaycaster"
        | "GraphicRaycaster"
        | "BaseInputModule"
        | "StandaloneInputModule"
        | "SerializedMonoBehaviour"
        | "NetworkBehaviour"
        | "NetworkManager"
        | "Mask"
        | "RectMask2D" => &["MonoBehaviour"],
        "SerializedScriptableObject"
        | "PlayableAsset"
        | "TrackAsset"
        | "TileBase"
        | "Tile"
        | "RuleTile"
        | "StateMachineBehaviour"
        | "ScriptableSingleton"
        | "ScriptableRendererFeature"
        | "RenderPipelineAsset"
        | "Editor"
        | "EditorWindow"
        | "ScriptableWizard"
        | "OdinEditor"
        | "OdinEditorWindow"
        | "OdinMenuEditorWindow" => &["ScriptableObject"],
        "OdinEditorWindowBase" => &["EditorWindow"],
        "UnityEditor.Editor" => &["Editor"],
        _ => &[],
    }
}

/// Unity's own lineage of a project-facing base: `Button` → MonoBehaviour, `OdinEditor` →
/// ScriptableObject + Editor, …
pub fn lineage(base: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let mut stack: Vec<&'static str> = implied_bases(base).to_vec();
    if let Some(own) = KNOWN_BASES.iter().find(|b| **b == base) {
        out.push(own);
    }
    // Odin editors are Editors / EditorWindows too.
    match base {
        "OdinEditor" => stack.push("Editor"),
        "OdinEditorWindow" | "OdinMenuEditorWindow" => stack.push("EditorWindow"),
        _ => {}
    }
    while let Some(b) = stack.pop() {
        if !out.contains(&b) {
            out.push(b);
            stack.extend(implied_bases(b).iter().copied());
        }
    }
    out
}

const KNOWN_BASES: &[&str] = &[
    "MonoBehaviour",
    "ScriptableObject",
    "Editor",
    "EditorWindow",
    "PropertyDrawer",
    "DecoratorDrawer",
    "AssetPostprocessor",
    "StateMachineBehaviour",
    "ScriptableWizard",
    "NetworkBehaviour",
];

/// Types whose instances live in scenes / assets: public members may be wired from the
/// inspector (UnityEvents, serialized references) without any code reference.
pub const SERIALIZED_ROOTS: &[&str] = &["MonoBehaviour", "ScriptableObject", "StateMachineBehaviour"];

/// Could `name` be a Unity message on some base? (cheap pre-filter at extraction time)
pub fn is_message_name(name: &str) -> bool {
    KNOWN_BASES.iter().any(|b| messages_of(b).contains(&name)) || INTERFACE_MESSAGES.iter().any(|(_, m)| m.contains(&name))
}

/// Methods Unity calls by *string* name (`Invoke("Foo")`, `StartCoroutine("Foo")`).
pub const BY_NAME_CALLS: &[&str] = &[
    "StartCoroutine",
    "StopCoroutine",
    "Invoke",
    "InvokeRepeating",
    "CancelInvoke",
    "IsInvoking",
    "SendMessage",
    "SendMessageUpwards",
    "BroadcastMessage",
];

/// Attributes on fields whose string arguments name methods of the same class
/// (`[ContextMenuItem("Reset", "ResetValue")]`, Odin `[OnValueChanged("Refresh")]`).
pub fn method_name_attr_args(attr: &str) -> Option<&'static [usize]> {
    match attr {
        // second argument is the method
        "ContextMenuItem" => Some(&[1]),
        // first argument is the method / condition member
        "OnValueChanged"
        | "ShowIf"
        | "HideIf"
        | "EnableIf"
        | "DisableIf"
        | "ValidateInput"
        | "ValueDropdown"
        | "OnInspectorInit"
        | "OnInspectorGUI"
        | "InlineButton"
        | "CustomValueDrawer"
        | "OnCollectionChanged" => Some(&[0]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lineage_reaches_mono_behaviour() {
        assert!(lineage("Button").contains(&"MonoBehaviour"));
        assert!(lineage("MonoBehaviour").contains(&"MonoBehaviour"));
        assert!(lineage("OdinEditor").contains(&"Editor"));
        assert!(messages_of("PropertyDrawer").contains(&"GetPropertyHeight"));
        assert!(is_message_name("OnPointerClick") && !is_message_name("Run"));
    }
}
