import type { WorkbenchDocumentV1 } from "../../types";

type Awaitable<T> = T | Promise<T>;

export type SurfaceCloseDeepReadonly<T> =
  T extends (...args: never[]) => unknown
    ? T
    : T extends readonly (infer TItem)[]
      ? readonly SurfaceCloseDeepReadonly<TItem>[]
      : T extends object
        ? { readonly [TKey in keyof T]: SurfaceCloseDeepReadonly<T[TKey]> }
        : T;

export type SurfaceCloseChoice = "save" | "discard" | "cancel";
export type SurfaceCloseResult = "allow" | "cancel";
export type SurfaceCloseGeneration = string | number;
export type SurfaceCloseSnapshot = SurfaceCloseDeepReadonly<WorkbenchDocumentV1>;

/** Immutable Workbench state captured before a close transaction is prepared. */
export interface SurfaceCloseContext {
  readonly snapshot: SurfaceCloseSnapshot;
  readonly transaction_version: number;
  readonly closing_surface_ids: readonly string[];
}

/** Exact resource state used to decide whether this close removes its final presentation. */
export interface SurfaceCloseResource {
  readonly resource_id: string;
  readonly resource_generation: SurfaceCloseGeneration;
  readonly presentation_ids: readonly string[];
}

export interface SurfaceClosePreparation extends SurfaceCloseResource {
  readonly choice: SurfaceCloseChoice;
  readonly save?: () => Awaitable<boolean>;
  readonly discard?: () => Awaitable<void>;
}

export interface SurfaceClosePreparationRequest {
  readonly context: SurfaceCloseContext;
  readonly resource: SurfaceCloseResource;
}

export interface SurfaceCloseRevalidationRequest {
  readonly context: SurfaceCloseContext;
  readonly resources: readonly SurfaceCloseResource[];
}

export interface SurfaceCloseCoordinatorOptions {
  readonly context: SurfaceCloseContext;
  /**
   * Resource observations may repeat when several closing surfaces present the
   * same resource. Every observation must carry the complete exact membership.
   */
  readonly resources: readonly SurfaceCloseResource[];
  readonly prepare_resource: (
    request: SurfaceClosePreparationRequest,
  ) => Awaitable<SurfaceClosePreparation | null>;
  readonly revalidate: (
    request: SurfaceCloseRevalidationRequest,
  ) => Awaitable<boolean>;
  readonly commit_layout: (context: SurfaceCloseContext) => Awaitable<boolean>;
}

function deepFreeze<T>(value: T, seen = new WeakSet<object>()): T {
  if ((typeof value !== "object" && typeof value !== "function") || value === null) {
    return value;
  }
  if (seen.has(value)) return value;
  seen.add(value);
  for (const key of Reflect.ownKeys(value)) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (descriptor && "value" in descriptor) deepFreeze(descriptor.value, seen);
  }
  return Object.freeze(value);
}

function captureContext(context: SurfaceCloseContext): SurfaceCloseContext {
  return Object.freeze({
    snapshot: deepFreeze(context.snapshot),
    transaction_version: context.transaction_version,
    closing_surface_ids: Object.freeze([...context.closing_surface_ids]),
  });
}

function isValidGeneration(value: SurfaceCloseGeneration): boolean {
  return typeof value === "string"
    ? value.length > 0
    : Number.isFinite(value) && !Object.is(value, -0);
}

function isValidResource(resource: SurfaceCloseResource): boolean {
  if (resource.resource_id.length === 0 || !isValidGeneration(resource.resource_generation)) {
    return false;
  }
  const presentationIds = new Set<string>();
  for (const presentationId of resource.presentation_ids) {
    if (presentationId.length === 0 || presentationIds.has(presentationId)) return false;
    presentationIds.add(presentationId);
  }
  return presentationIds.size > 0;
}

function hasSamePresentationMembership(
  left: readonly string[],
  right: readonly string[],
): boolean {
  if (left.length !== right.length) return false;
  const rightIds = new Set(right);
  return left.every((presentationId) => rightIds.has(presentationId));
}

function matchesResource(
  preparation: SurfaceClosePreparation,
  resource: SurfaceCloseResource,
): boolean {
  return preparation.resource_id === resource.resource_id
    && Object.is(preparation.resource_generation, resource.resource_generation)
    && hasSamePresentationMembership(
      preparation.presentation_ids,
      resource.presentation_ids,
    );
}

function hasValidChoiceEffect(preparation: SurfaceClosePreparation): boolean {
  if (preparation.choice === "save") return typeof preparation.save === "function";
  if (preparation.choice === "discard") return typeof preparation.discard === "function";
  return preparation.choice === "cancel";
}

function groupFinalClosingResources(
  context: SurfaceCloseContext,
  observations: readonly SurfaceCloseResource[],
): readonly SurfaceCloseResource[] | null {
  const closingSurfaceIds = new Set(context.closing_surface_ids);
  if (closingSurfaceIds.size !== context.closing_surface_ids.length) return null;

  const presentationOwners = new Map<string, string>();
  const grouped = new Map<string, SurfaceCloseResource>();
  for (const observation of observations) {
    if (!isValidResource(observation)) return null;

    for (const presentationId of observation.presentation_ids) {
      const owner = presentationOwners.get(presentationId);
      if (owner !== undefined && owner !== observation.resource_id) return null;
      presentationOwners.set(presentationId, observation.resource_id);
    }

    const existing = grouped.get(observation.resource_id);
    if (existing) {
      if (
        !Object.is(existing.resource_generation, observation.resource_generation)
        || !hasSamePresentationMembership(
          existing.presentation_ids,
          observation.presentation_ids,
        )
      ) return null;
      continue;
    }
    grouped.set(observation.resource_id, observation);
  }

  return [...grouped.values()].filter((resource) => (
    resource.presentation_ids.every((presentationId) => closingSurfaceIds.has(presentationId))
  ));
}

/**
 * Coordinates a close as choice collection, pre-effect revalidation, saves,
 * one layout commit, and post-commit discard/release cleanup.
 */
export async function coordinateSurfaceClose(
  options: SurfaceCloseCoordinatorOptions,
): Promise<SurfaceCloseResult> {
  let context: SurfaceCloseContext;
  try {
    context = captureContext(options.context);
  } catch {
    return "cancel";
  }

  const resources = groupFinalClosingResources(context, options.resources);
  if (resources === null) return "cancel";

  const preparations: SurfaceClosePreparation[] = [];
  let preparationFailed = false;
  for (const resource of resources) {
    try {
      const preparation = await options.prepare_resource({
        context,
        resource,
      });
      if (preparation === null) {
        preparationFailed = true;
        continue;
      }
      if (!matchesResource(preparation, resource) || !hasValidChoiceEffect(preparation)) {
        preparationFailed = true;
        continue;
      }
      preparations.push(preparation);
    } catch {
      preparationFailed = true;
    }
  }

  if (
    preparationFailed
    || preparations.some((preparation) => preparation.choice === "cancel")
  ) return "cancel";

  const revalidationResources = preparations.map((preparation) => ({
    resource_id: preparation.resource_id,
    resource_generation: preparation.resource_generation,
    presentation_ids: preparation.presentation_ids,
  }));
  try {
    if (!await options.revalidate({
      context,
      resources: revalidationResources,
    })) return "cancel";
  } catch {
    return "cancel";
  }

  for (const preparation of preparations) {
    if (preparation.choice !== "save") continue;
    try {
      if (!await preparation.save?.()) return "cancel";
    } catch {
      return "cancel";
    }
  }

  try {
    if (!await options.commit_layout(context)) return "cancel";
  } catch {
    return "cancel";
  }

  for (const preparation of preparations) {
    if (preparation.choice === "discard") await preparation.discard?.();
  }
  return "allow";
}
