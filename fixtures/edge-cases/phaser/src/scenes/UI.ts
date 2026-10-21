import Phaser from 'phaser';

export class UI extends Phaser.Scene {
  constructor() {
    super('UI');
  }

  create() {
    const game = this.scene.get('Game');
    game.events.on('score', this.onScore, this);
    game.restartLevel();
    this.registry.get('score');
  }

  onScore(points: number) {
    this.showScore(points);
  }

  showScore(points: number) {}
}
