export interface PackageDetail {
	name: string;
	distro: string;
	latest_version: string;
	description: string | null;
	versions: VersionSummary[];
	dependencies: string[];
	download_count: number;
	download_count_30d: number;
	size_bytes: number;
	license: string | null;
	homepage: string | null;
	converted: boolean;
}

export interface VersionSummary {
	version: string;
	architecture: string | null;
	size: number;
	converted: boolean;
}

export interface SearchResult {
	name: string;
	version: string;
	distro: string;
	description: string | null;
	size: number;
	converted: boolean;
	score: number;
}

export interface SearchResponse {
	results: SearchResult[];
	total: number;
	query: string;
}

export interface SparseIndexEntry {
	name: string;
	distro: string;
	versions: SparseVersionEntry[];
}

export interface SparseVersionEntry {
	version: string;
	dependencies: string | null;
	provides: string | null;
	architecture: string | null;
	size: number;
	converted: boolean;
	content_hash: string | null;
}

export interface StatsOverview {
	total_packages: number;
	total_downloads: number;
	total_distros: number;
	downloads_30d: number;
	total_converted: number;
}

export interface PopularPackage {
	name: string;
	distro: string;
	version: string;
	description: string | null;
	download_count: number;
	size: number;
}

export interface RecentPackage {
	name: string;
	distro: string;
	version: string;
	description: string | null;
	download_count: number;
	size: number;
}

export interface PackageListResponse {
	distro: string;
	packages: string[];
	total: number;
	page: number;
	per_page: number;
}

export interface SuggestResponse {
	suggestions: string[];
}
