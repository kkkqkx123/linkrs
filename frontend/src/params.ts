import { defineParams } from '@sveltejs/kit/params';

/** Route matchers shared by the studio pages. */
export const params = defineParams({
	/** Schema sub-page tabs: /schema/spaces, /schema/tags, ... */
	schemaTab: (tab) => {
		const value = Array.isArray(tab) ? tab[0] : tab;
		return [
			'spaces',
			'tags',
			'edges',
			'indexes',
			'visualization',
			'functions',
			'versions',
		].includes(value)
			? value
			: undefined;
	},
});
