import Phaser from 'phaser';
import { Player } from '../objects/Player';
import { Enemy } from '../objects/Enemy';

export class Game extends Phaser.Scene {
  private player!: Player;

  constructor() {
    super({ key: 'Game' });
  }

  init() {}

  create() {
    this.player = new Player(this, 10, 10);
    const enemies = this.physics.add.group({ classType: Enemy });
    const first = enemies.get(5, 5);
    first.patrol();
    this.physics.add.collider(this.player, enemies, this.onHit, undefined, this);
    this.time.delayedCall(500, this.spawn, [], this);
    this.anims.create({ key: 'walk', frames: this.anims.generateFrameNumbers('enemy', { start: 0, end: 3 }) });
    this.scene.launch('UI');
    this.registry.set('score', 0);
  }

  update() {
    this.player.move();
  }

  restartLevel(): void {}

  private onHit() {
    this.events.emit('score', 10);
    this.sound.play('jump');
    this.sound.play('missing-sfx');
  }

  private spawn() {
    this.add.sprite(1, 1, 'player').play('walk');
  }

  private unusedHelper() {}
}
