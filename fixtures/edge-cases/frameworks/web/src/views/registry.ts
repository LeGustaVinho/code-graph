// Every view is loaded by glob; none is imported by name.
export const views = import.meta.glob('./*.tsx', { eager: true });
