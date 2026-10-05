/**
 * Mock scenario switches, read from URL query params
 * (`?scenario=slow|error|empty`) or localStorage (`mockScenario`).
 * They let loading, error and empty states be exercised without touching
 * component code.
 */

export type Scenario = 'normal' | 'slow' | 'error' | 'empty';

let cached: Scenario | null = null;

export function currentScenario(): Scenario {
	if (cached) return cached;
	let value: string | null = null;
	try {
		value = new URLSearchParams(window.location.search).get('scenario');
		if (!value) value = localStorage.getItem('mockScenario');
	} catch {
		/* no browser storage available */
	}
	const scenario: Scenario =
		value === 'slow' || value === 'error' || value === 'empty'
			? value
			: 'normal';
	cached = scenario;
	return scenario;
}

/** Simulated network latency, inflated under the `slow` scenario. */
export function mockDelay(baseMs = 120): Promise<void> {
	const ms = currentScenario() === 'slow' ? Math.max(baseMs, 1500) : baseMs;
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** True when the `error` scenario is active and this call should fail. */
export function scenarioWantsError(): boolean {
	return currentScenario() === 'error';
}

/** True when the `empty` scenario is active: collections render empty. */
export function scenarioIsEmpty(): boolean {
	return currentScenario() === 'empty';
}
