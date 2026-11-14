/* Backtracking browser demo: capture tap.
   Collects the stereo input in 1024-frame blocks and posts them to the page, which keeps the
   retained history in a ring buffer. The node passes nothing through (its output stays silent). */
class BTCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = 1024;
    this.n = 0;
    this.L = new Float32Array(this.size);
    this.R = new Float32Array(this.size);
  }
  process(inputs) {
    const input = inputs[0];
    const l = input && input[0];
    const r = input && (input[1] || input[0]);
    const len = l ? l.length : 128;
    if (l) {
      this.L.set(l, this.n);
      this.R.set(r, this.n);
    }
    // with no connected input the block stays zero-filled
    this.n += len;
    if (this.n >= this.size) {
      this.port.postMessage({ l: this.L, r: this.R }, [this.L.buffer, this.R.buffer]);
      this.L = new Float32Array(this.size);
      this.R = new Float32Array(this.size);
      this.n = 0;
    }
    return true;
  }
}
registerProcessor("bt-capture", BTCapture);
