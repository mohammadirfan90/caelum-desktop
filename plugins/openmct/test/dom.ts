// SPDX-License-Identifier: Apache-2.0
// Test equipment only: a jsdom window per test file, loaded in a hook (see vitest.config.ts).
import { afterEach, beforeAll } from 'vitest';

export interface TestDom {
  readonly window: Window & typeof globalThis;
  readonly document: Document;
  /** A fresh container attached to the document body. */
  container(): HTMLElement;
}

/**
 * Registers hooks that load jsdom once per file and empty the body after each test. The
 * returned getter throws if used before `beforeAll` ran.
 */
export function useDom(): () => TestDom {
  let dom: TestDom | null = null;
  beforeAll(async () => {
    const { JSDOM } = await import('jsdom');
    const instance = new JSDOM('<!doctype html><html><body></body></html>', {
      pretendToBeVisual: true,
    });
    const window = instance.window;
    const document = window.document;
    dom = {
      window,
      document,
      container: () => {
        const el = document.createElement('div');
        document.body.append(el);
        return el;
      },
    };
  });
  afterEach(() => {
    dom?.document.body.replaceChildren();
  });
  return () => {
    if (dom === null) throw new Error('jsdom not loaded yet');
    return dom;
  };
}
