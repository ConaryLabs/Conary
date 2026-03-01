import type {
	PackageDetail,
	VersionSummary,
	SearchResponse,
	StatsOverview,
	PopularPackage,
	RecentPackage,
	SparseIndexEntry,
	PackageListResponse,
	SuggestResponse
} from './types';

const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '';

async function fetchJson<T>(path: string): Promise<T> {
	const response = await fetch(`${BASE_URL}${path}`);
	if (!response.ok) {
		throw new Error(`API error: ${response.status} ${response.statusText}`);
	}
	return response.json() as Promise<T>;
}

export async function searchPackages(
	q: string,
	distro?: string,
	limit: number = 20
): Promise<SearchResponse> {
	const params = new URLSearchParams({ q, limit: String(limit) });
	if (distro) params.set('distro', distro);
	return fetchJson(`/v1/search?${params}`);
}

export async function suggestPackages(
	prefix: string,
	limit: number = 10
): Promise<SuggestResponse> {
	const params = new URLSearchParams({ prefix, limit: String(limit) });
	return fetchJson(`/v1/suggest?${params}`);
}

export async function getPackageDetail(
	distro: string,
	name: string
): Promise<PackageDetail> {
	return fetchJson(`/v1/packages/${encodeURIComponent(distro)}/${encodeURIComponent(name)}`);
}

export async function getPackageVersions(
	distro: string,
	name: string
): Promise<VersionSummary[]> {
	return fetchJson(`/v1/packages/${encodeURIComponent(distro)}/${encodeURIComponent(name)}/versions`);
}

export async function getPackageDependencies(
	distro: string,
	name: string
): Promise<string[]> {
	return fetchJson(`/v1/packages/${encodeURIComponent(distro)}/${encodeURIComponent(name)}/dependencies`);
}

export async function getReverseDependencies(
	distro: string,
	name: string
): Promise<string[]> {
	return fetchJson(`/v1/packages/${encodeURIComponent(distro)}/${encodeURIComponent(name)}/rdepends`);
}

export async function getPopularPackages(
	distro?: string,
	limit: number = 50
): Promise<PopularPackage[]> {
	const params = new URLSearchParams({ limit: String(limit) });
	if (distro) params.set('distro', distro);
	return fetchJson(`/v1/stats/popular?${params}`);
}

export async function getRecentPackages(
	distro?: string,
	limit: number = 50
): Promise<RecentPackage[]> {
	const params = new URLSearchParams({ limit: String(limit) });
	if (distro) params.set('distro', distro);
	return fetchJson(`/v1/stats/recent?${params}`);
}

export async function getStatsOverview(): Promise<StatsOverview> {
	return fetchJson('/v1/stats/overview');
}

export async function listPackages(
	distro: string,
	page: number = 1,
	perPage: number = 100
): Promise<PackageListResponse> {
	const params = new URLSearchParams({
		page: String(page),
		per_page: String(perPage)
	});
	return fetchJson(`/v1/index/${encodeURIComponent(distro)}?${params}`);
}

export async function getSparseIndex(
	distro: string,
	name: string
): Promise<SparseIndexEntry> {
	return fetchJson(`/v1/index/${encodeURIComponent(distro)}/${encodeURIComponent(name)}`);
}
