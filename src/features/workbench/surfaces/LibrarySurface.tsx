import {
  LibraryView,
  type LibraryViewProps,
} from "../../../views/LibraryView";

export interface LibrarySurfaceProps extends Omit<LibraryViewProps, "surfaceId"> {
  surface_id: string;
}

/** Typed workbench presentation wrapper; the Library store remains the resource owner. */
export function LibrarySurface({ surface_id, ...viewProps }: LibrarySurfaceProps) {
  return (
    <section
      className="h-full min-h-0 min-w-0"
      data-surface-id={surface_id}
      data-testid="library-surface"
    >
      <LibraryView {...viewProps} surfaceId={surface_id} />
    </section>
  );
}
