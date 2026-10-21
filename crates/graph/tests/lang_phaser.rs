//! Phaser games (fixtures/edge-cases/phaser).

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/phaser")
}

fn entry(c: &rusqlite::Connection, qname: &str) -> String {
    sym(c, qname).entry_kind
}

#[test]
fn scenes_and_lifecycle_hooks() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(entry(c, "Game.create"), "phaser hook Scene.create");
    assert_eq!(entry(c, "Game.update"), "phaser hook Scene.update");
    assert_eq!(entry(c, "Boot.preload"), "phaser hook Scene.preload", "through the project's BaseScene");
    assert_eq!(entry(c, "Player.preUpdate"), "phaser hook Sprite.preUpdate");
    assert_eq!(entry(c, "Enemy.preUpdate"), "phaser hook Sprite.preUpdate", "Physics.Arcade.Sprite imported from phaser");
    assert_eq!(entry(c, "Game"), "phaser scene");
    assert_eq!(entry(c, "Game.unusedHelper"), "", "not a hook");
    assert_eq!(entry(c, "Game.restartLevel"), "");
}

#[test]
fn scene_transitions_and_scene_get() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Boot.create", "Game.init", "CALLS").is_some(), "this.scene.start('Game'): {:?}", targets(c, "Boot.create", "CALLS"));
    assert!(edge(c, "Game.create", "UI.create", "CALLS").is_some(), "this.scene.launch('UI'): {:?}", targets(c, "Game.create", "CALLS"));
    assert!(edge(c, "UI.create", "Game.restartLevel", "CALLS").is_some(), "this.scene.get('Game').restartLevel(): {:?}", targets(c, "UI.create", "CALLS"));
}

#[test]
fn callbacks_events_and_groups() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Game.create", "Game.onHit", "CALLS").is_some(), "collider callback");
    assert!(edge(c, "Game.create", "Game.spawn", "CALLS").is_some(), "delayedCall callback");
    assert!(edge(c, "Game.onHit", "UI.onScore", "CALLS").is_some(), "emit('score') → on('score', this.onScore): {:?}", targets(c, "Game.onHit", "CALLS"));
    assert_eq!(edge(c, "Game.create", "Enemy.patrol", "CALLS").as_deref(), Some("exact"), "group({{ classType: Enemy }}).get()");
}

#[test]
fn assets_animations_and_registry() {
    let f = fixture();
    let c = &f.conn;
    let usage = |key: &str, usage: &str| count(c, &format!("SELECT COUNT(*) FROM config_dependency WHERE config_key = '{key}' AND usage = '{usage}'"));
    assert_eq!(usage("asset:player", "define"), 1);
    assert_eq!(usage("asset:player", "read"), 2, "add.sprite + Player's super(scene, x, y, 'player')");
    assert_eq!(usage("asset:enemy", "read"), 1, "generateFrameNumbers('enemy')");
    assert_eq!(usage("asset:jump", "read"), 1, "sound.play('jump')");
    assert_eq!((usage("asset:missing-sfx", "define"), usage("asset:missing-sfx", "read")), (0, 1));
    assert_eq!(usage("asset:unused-bg", "read"), 0);
    assert_eq!((usage("anim:walk", "define"), usage("anim:walk", "read")), (1, 1));
    assert_eq!((usage("registry:score", "define"), usage("registry:score", "read")), (1, 1));
    assert_eq!(count(c, "SELECT COUNT(*) FROM state_mutation WHERE target_state = 'registry:score' AND mutation_type = 'game_registry'"), 1);
}
