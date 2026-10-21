import { Scene } from 'phaser';

// Project base scene: its subclasses' hooks are Phaser hooks too.
export abstract class BaseScene extends Scene {
  protected trace(): void {}
}
