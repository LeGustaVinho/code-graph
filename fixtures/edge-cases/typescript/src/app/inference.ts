export class Engine {
  ignite(): void {}
  self() {
    return this;
  }
}

const makeEngine = () => new Engine();

async function loadEngine() {
  const e = new Engine();
  return e;
}

// No return annotations: the types come from the function bodies.
export async function runEngine(): Promise<void> {
  makeEngine().ignite();
  const e = await loadEngine();
  e.self().ignite();
}
