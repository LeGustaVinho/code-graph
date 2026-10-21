export class Animal {
  speak(): void {}
}

export class Dog extends Animal {
  speak(): void {
    super.speak();
  }
}
