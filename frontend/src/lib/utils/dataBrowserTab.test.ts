import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	DEFAULT_DATA_BROWSER_TAB,
	parseDataBrowserTab,
	retrievalHitId,
} from './dataBrowserTab.ts';

describe('parseDataBrowserTab', () => {
	it('round-trips every supported tab', () => {
		for (const tab of ['vertices', 'edges', 'vector', 'fulltext']) {
			assert.equal(parseDataBrowserTab(tab), tab);
		}
	});

	it('falls back to the default for unknown, missing and non-string values', () => {
		assert.equal(parseDataBrowserTab('search'), DEFAULT_DATA_BROWSER_TAB);
		assert.equal(parseDataBrowserTab(''), DEFAULT_DATA_BROWSER_TAB);
		assert.equal(parseDataBrowserTab(null), DEFAULT_DATA_BROWSER_TAB);
		assert.equal(parseDataBrowserTab(undefined), DEFAULT_DATA_BROWSER_TAB);
		assert.equal(parseDataBrowserTab(42), DEFAULT_DATA_BROWSER_TAB);
	});
});

describe('retrievalHitId', () => {
	it('prefers id over alias columns', () => {
		assert.equal(
			retrievalHitId({ id: 'n1', doc_id: 'n2', vid: 'n3' }),
			'n1',
		);
	});

	it('falls back to document and vertex id aliases', () => {
		assert.equal(retrievalHitId({ doc_id: 'd9' }), 'd9');
		assert.equal(retrievalHitId({ vid: 'v3' }), 'v3');
		assert.equal(retrievalHitId({ vertex_id: 'v4' }), 'v4');
	});

	it('accepts finite numeric ids', () => {
		assert.equal(retrievalHitId({ id: 7 }), '7');
	});

	it('returns empty for missing, blank or non-finite ids', () => {
		assert.equal(retrievalHitId({ score: 0.9 }), '');
		assert.equal(retrievalHitId({ id: '   ' }), '');
		assert.equal(retrievalHitId({ id: Number.NaN }), '');
	});
});
