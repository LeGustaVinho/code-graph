export function outer(logger) {
  function emit(message) {
    logger(message);
  }
  const twice = () => {
    emit('first');
    emit('second');
  };
  twice();
}
