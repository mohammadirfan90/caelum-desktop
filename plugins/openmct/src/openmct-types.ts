// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * The minimal subset of the Open MCT API this plugin uses (F-GND-04). Declared locally so the
 * plugin does not depend on the full `openmct` package; the host application passes its own
 * Open MCT instance to `install()`. Shapes follow the Open MCT 4.x public API (types, objects,
 * telemetry, objectViews, time). Verification against a live Open MCT is an open item (README).
 */

export interface Identifier {
  readonly namespace: string;
  readonly key: string;
}

/** Open MCT enumeration entry for `format: 'enum'` telemetry values. */
export interface EnumerationEntry {
  readonly value: number;
  readonly string: string;
}

/** Telemetry value metadata (the entries of `domainObject.telemetry.values`). */
export interface ValueMetadata {
  readonly key: string;
  readonly name: string;
  readonly source?: string;
  readonly format?: string;
  readonly hints?: { readonly domain?: number; readonly range?: number };
  readonly enumerations?: readonly EnumerationEntry[];
}

export interface DomainObject {
  readonly identifier: Identifier;
  readonly type: string;
  readonly name: string;
  readonly location?: string;
  readonly composition?: readonly Identifier[];
  readonly telemetry?: { readonly values: readonly ValueMetadata[] };
  /** CAELUM-specific properties (only the crew id of a crew-status object). */
  readonly caelum?: { readonly crewId: string };
}

export interface TypeDefinition {
  readonly name: string;
  readonly description?: string;
  readonly cssClass?: string;
  readonly creatable?: boolean;
}

export interface ObjectProvider {
  get(identifier: Identifier): Promise<DomainObject | undefined>;
}

export interface TelemetryRequestOptions {
  readonly start?: number;
  readonly end?: number;
  readonly signal?: AbortSignal;
}

/** One telemetry datum handed to Open MCT. Values are numbers or strings only. */
export type Datum = Readonly<Record<string, number | string>>;

/** Open MCT staleness response (`telemetry.isStale` / `subscribeToStaleness`). */
export interface StalenessResponse {
  readonly isStale: boolean;
  /** Evaluation time, epoch ms. */
  readonly utc: number;
}

export interface TelemetryProvider {
  supportsRequest(domainObject: DomainObject): boolean;
  request(domainObject: DomainObject, options?: TelemetryRequestOptions): Promise<Datum[]>;
  supportsSubscribe(domainObject: DomainObject): boolean;
  subscribe(domainObject: DomainObject, callback: (datum: Datum) => void): () => void;
  /** Optional Open MCT staleness API: lets native Open MCT views mark old status as stale. */
  supportsStaleness?(domainObject: DomainObject): boolean;
  isStale?(domainObject: DomainObject): Promise<StalenessResponse>;
  subscribeToStaleness?(
    domainObject: DomainObject,
    callback: (response: StalenessResponse) => void,
  ): () => void;
}

export interface View {
  show(element: HTMLElement): void;
  destroy(): void;
}

export interface ViewProvider {
  readonly key: string;
  readonly name: string;
  readonly cssClass?: string;
  canView(domainObject: DomainObject): boolean;
  view(domainObject: DomainObject): View;
}

export interface TimeBounds {
  readonly start: number;
  readonly end: number;
}

/** `boundsChanged` listener; `isTick` is true for clock ticks in realtime mode. */
export type BoundsListener = (bounds: TimeBounds, isTick: boolean) => void;

export interface OpenMctLike {
  readonly types: { addType(key: string, definition: TypeDefinition): void };
  readonly objects: {
    addRoot(identifier: Identifier): void;
    addProvider(namespace: string, provider: ObjectProvider): void;
  };
  readonly telemetry: { addProvider(provider: TelemetryProvider): void };
  readonly objectViews: { addProvider(provider: ViewProvider): void };
  readonly time: TimeApiLike;
}

export interface TimeApiLike {
  getBounds(): TimeBounds;
  on(event: 'boundsChanged', callback: BoundsListener): void;
  off(event: 'boundsChanged', callback: BoundsListener): void;
}
