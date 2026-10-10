// SPDX-License-Identifier: Apache-2.0
// Minimal local typing for the jsdom test dependency (jsdom ships no types; no @types package
// is added for test-only use).
declare module 'jsdom' {
  export class JSDOM {
    constructor(html?: string, options?: { readonly pretendToBeVisual?: boolean });
    readonly window: Window & typeof globalThis;
  }
}
