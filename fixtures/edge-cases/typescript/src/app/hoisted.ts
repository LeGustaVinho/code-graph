// Export lists may precede the (hoisted) declarations they name.
export { early, other as renamed };
export default Late;

function early(): void {}

function other(): void {}

class Late {
  run(): void {}
}
