import Phaser from 'phaser';
import { Boot } from './scenes/Boot';
import { Game } from './scenes/Game';
import { UI } from './scenes/UI';

const config: Phaser.Types.Core.GameConfig = { type: Phaser.AUTO, scene: [Boot, Game, UI] };

export const game = new Phaser.Game(config);
