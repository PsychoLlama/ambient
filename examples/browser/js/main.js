// The native side of `src/main.ab` that is written by hand: a function
// for each extern with no binding, by its name. A build of the package
// takes the ones its program reaches.

/** Answers after a number of milliseconds: its extern is `suspending`. */
export function slept(milliseconds) {
  return new Promise((wake) => setTimeout(wake, milliseconds, null));
}
