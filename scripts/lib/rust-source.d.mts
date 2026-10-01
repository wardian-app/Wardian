export const IDENT: 1;
export const PUNCT: 2;
export const STR: 3;
export const OTHER: 4;

export interface Tokens {
  kinds: number[];
  values: string[];
  lines: number[];
  count: number;
}

export interface SourceAnalysis {
  tokens: Tokens;
  match: Int32Array;
  testOnly: Uint8Array;
  inactive: Uint8Array;
  inactiveRegions: Array<[number, number]>;
}

export interface CfgEnv {
  flag(name: string): boolean | undefined;
  kv(name: string, value: string): boolean | undefined;
}

export interface Reference {
  name: string;
  index: number;
  testOnly: boolean;
  inactive: boolean;
}

export interface PublicItem {
  name: string;
  kind: string;
  line: number;
  start: number;
  end: number;
}

export function lexRust(source: string): Tokens;
export function productionCfgEnv(platform?: string): CfgEnv;
export function itemEnd(tokens: Tokens, match: Int32Array, start: number, limit?: number): number;
export function analyzeSource(
  source: string,
  inherited: { testOnly: boolean; inactive: boolean },
  env: CfgEnv,
): SourceAnalysis;
export function inactiveRegionAt(analysis: SourceAnalysis, index: number): [number, number] | undefined;
export function loadCrate(
  rootFile: string,
  env: CfgEnv,
  read?: (file: string) => string,
  exists?: (file: string) => boolean,
): { files: Map<string, SourceAnalysis>; unresolved: string[] };
export function collectReferences(analysis: SourceAnalysis, names: Set<string>): Reference[];
export function publicItems(analysis: SourceAnalysis): PublicItem[];
