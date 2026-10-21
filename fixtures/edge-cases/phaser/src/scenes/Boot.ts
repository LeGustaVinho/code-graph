import { BaseScene } from './BaseScene';

export class Boot extends BaseScene {
  constructor() {
    super('Boot');
  }

  preload() {
    this.load.image('player', 'assets/player.png');
    this.load.spritesheet('enemy', 'assets/enemy.png', { frameWidth: 16 });
    this.load.audio('jump', 'assets/jump.ogg');
    this.load.image('unused-bg', 'assets/bg.png');
  }

  create() {
    this.scene.start('Game');
  }
}
