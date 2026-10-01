import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  AUTO_STREAM_THRESHOLD_MAX,
  AUTO_STREAM_THRESHOLD_MIN,
  clampAutoThreshold,
  DEFAULT_AUTO_STREAM_THRESHOLD,
  resolveAutoPath,
} from './autoRoute.ts';

describe('resolveAutoPath', () => {
  it('streams above the threshold and materializes at or below it', () => {
    assert.equal(resolveAutoPath({ mode: 'single', estimatedRows: 1001 }, 1000), 'stream');
    assert.equal(resolveAutoPath({ mode: 'single', estimatedRows: 1000 }, 1000), 'materialized');
    assert.equal(resolveAutoPath({ mode: 'single', estimatedRows: 0 }, 1000), 'materialized');
  });

  it('streams when the estimate is missing or the script is a batch', () => {
    assert.equal(resolveAutoPath({ mode: 'single', estimatedRows: null }, 1000), 'stream');
    assert.equal(resolveAutoPath({ mode: 'single', estimatedRows: undefined }, 1000), 'stream');
    assert.equal(resolveAutoPath({ mode: 'batch', estimatedRows: 3 }, 1_000_000), 'stream');
    assert.equal(resolveAutoPath({ mode: null, estimatedRows: 1 }, 1000), 'stream');
  });
});

describe('clampAutoThreshold', () => {
  it('clamps into bounds and repairs non-finite input', () => {
    assert.equal(clampAutoThreshold(0), AUTO_STREAM_THRESHOLD_MIN);
    assert.equal(clampAutoThreshold(-5), AUTO_STREAM_THRESHOLD_MIN);
    assert.equal(clampAutoThreshold(10_000_001), AUTO_STREAM_THRESHOLD_MAX);
    assert.equal(clampAutoThreshold(2500.9), 2500);
    assert.equal(clampAutoThreshold(NaN), DEFAULT_AUTO_STREAM_THRESHOLD);
    assert.equal(clampAutoThreshold(Infinity), DEFAULT_AUTO_STREAM_THRESHOLD);
  });
});
