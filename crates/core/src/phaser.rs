//! Phaser engine knowledge: the methods Phaser invokes by name on scenes, game objects and
//! plugins, the loader / factory APIs that define and use asset keys, and the APIs that take
//! callbacks the engine calls later.
//!
//! Extraction only marks *candidates* (a method named like a hook, in a class with bases); the
//! global pass in `codegraph_semantic::phaser` confirms them by walking the real inheritance chain
//! (project base classes included) up to a Phaser base.

const SCENE: &[&str] = &["init", "preload", "create", "update"];
const GAME_OBJECT: &[&str] = &["preUpdate", "preDestroy"];
const PLUGIN: &[&str] = &["boot", "init", "start", "stop", "destroy"];

/// Phaser base classes by their short name (`Phaser.Physics.Arcade.Sprite` → `Sprite`).
const GAME_OBJECTS: &[&str] = &[
    "GameObject",
    "Sprite",
    "Image",
    "Container",
    "Text",
    "BitmapText",
    "Graphics",
    "Zone",
    "TileSprite",
    "Rectangle",
    "Arc",
    "Ellipse",
    "Polygon",
    "Shape",
    "NineSlice",
    "Video",
    "DOMElement",
    "Layer",
];

/// Hooks Phaser calls on instances of `base` (a short Phaser class name).
pub fn hooks_of(base: &str) -> &'static [&'static str] {
    match base {
        "Scene" => SCENE,
        "BasePlugin" | "ScenePlugin" => PLUGIN,
        b if GAME_OBJECTS.contains(&b) => GAME_OBJECT,
        _ => &[],
    }
}

pub fn is_hook_name(name: &str) -> bool {
    SCENE.contains(&name) || GAME_OBJECT.contains(&name) || PLUGIN.contains(&name)
}

/// `this.load.<method>(key, url)`: the asset kind a loader method defines.
pub fn loader_kind(method: &str) -> Option<&'static str> {
    Some(match method {
        "image" | "svg" | "spritesheet" | "atlas" | "multiatlas" | "aseprite" | "unityAtlas" | "texture" => "texture",
        "audio" | "audioSprite" => "audio",
        "tilemapTiledJSON" | "tilemapCSV" | "tilemapImpact" => "tilemap",
        "json" | "xml" | "text" | "binary" | "glsl" | "html" | "css" | "script" | "plugin" | "scenePlugin" => "data",
        "bitmapFont" => "texture",
        "video" => "video",
        "pack" => "pack",
        "animation" => "anim",
        _ => return None,
    })
}

/// Factory methods taking a texture key, and the argument it is at: `add.sprite(x, y, key)`.
pub fn texture_arg(method: &str) -> Option<usize> {
    Some(match method {
        "sprite" | "image" | "bitmapText" | "particles" | "video" | "nineslice" | "plane" | "rope" | "mesh" | "staticImage"
        | "staticSprite" => 2,
        "tileSprite" => 4,
        "setTexture" | "generateFrameNumbers" | "generateFrameNames" | "exists" => 0,
        _ => return None,
    })
}

/// Engine APIs whose function arguments the engine calls back later, and which arguments.
pub fn callback_args(method: &str) -> &'static [usize] {
    match method {
        // physics.add.collider(a, b, collideCallback, processCallback, ctx)
        "collider" | "overlap" => &[2, 3],
        // time.delayedCall(delay, callback, args, scope)
        "delayedCall" => &[1],
        // events.on('x', fn, ctx) / input.keyboard.on('keydown-SPACE', fn) / once / addListener
        "on" | "once" | "addListener" => &[1],
        // setInteractive().on(…) is covered by `on`; `forEach` style group callbacks:
        "iterate" | "each" | "forEach" | "iterateLocal" => &[0],
        _ => &[],
    }
}

/// Option keys holding callbacks: `time.addEvent({ callback })`, `tweens.add({ onComplete })`.
pub const CALLBACK_KEYS: &[&str] = &["callback", "onComplete", "onUpdate", "onStart", "onYoyo", "onRepeat", "onLoop", "onStop", "onActive"];

/// Scene manager methods that run another scene by key.
pub const SCENE_TRANSITIONS: &[&str] = &["start", "launch", "switch", "run", "restart", "wake", "resume", "bringToTop", "moveAbove", "moveBelow"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks() {
        assert!(hooks_of("Scene").contains(&"create"));
        assert!(hooks_of("Sprite").contains(&"preUpdate"));
        assert!(hooks_of("Foo").is_empty());
        assert_eq!(loader_kind("spritesheet"), Some("texture"));
        assert_eq!(texture_arg("tileSprite"), Some(4));
    }
}
