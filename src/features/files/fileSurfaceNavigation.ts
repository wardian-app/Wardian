import type { WorkbenchNavigationService } from "../workbench/navigationService";
import { createFileSurfaceState, fileResourceKey } from "./fileResourceKey";

/** Opens a file permanently, pinning an existing transient preview in place. */
export function openPermanentFileSurface(
  navigation: WorkbenchNavigationService,
  path: string,
): string {
  const surfaceId = navigation.open({
    surface_type: "files",
    resource_key: fileResourceKey(path),
    state: createFileSurfaceState(false),
  });
  navigation.pin_transient(surfaceId);
  return surfaceId;
}
