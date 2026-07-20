export type PreviewTarget = {
	id: 'fedora' | 'ubuntu' | 'arch';
	name: string;
	profile: string;
	asset: string;
	installCommand: string;
};

const version = '0.11.3';
const tag = 'v0.11.3';
const releaseUrl = `https://github.com/ConaryLabs/Conary/releases/tag/${tag}`;
const downloadBaseUrl = `https://github.com/ConaryLabs/Conary/releases/download/${tag}`;

export const previewRelease = {
	version,
	tag,
	releaseUrl,
	downloadBaseUrl,
	testerGuideUrl: `https://github.com/ConaryLabs/Conary/blob/${tag}/docs/guides/agent-assisted-tester-loop.md`,
	feedbackUrl: 'https://github.com/ConaryLabs/Conary/issues/new?template=beta_feedback.md',
	workDirectory: `$HOME/conary-preview-${tag}`,
	targets: [
		{
			id: 'fedora',
			name: 'Fedora 44',
			profile: 'fedora-44',
			asset: 'conary-0.11.3-1.fc44.x86_64.rpm',
			installCommand: 'sudo dnf install ./conary-0.11.3-1.fc44.x86_64.rpm'
		},
		{
			id: 'ubuntu',
			name: 'Ubuntu 26.04 LTS',
			profile: 'ubuntu-26.04',
			asset: 'conary_0.11.3-1_amd64.deb',
			installCommand: 'sudo apt install ./conary_0.11.3-1_amd64.deb'
		},
		{
			id: 'arch',
			name: 'Arch Linux',
			profile: 'arch',
			asset: 'conary-0.11.3-1-x86_64.pkg.tar.zst',
			installCommand: 'sudo pacman -U ./conary-0.11.3-1-x86_64.pkg.tar.zst'
		}
	] satisfies PreviewTarget[]
} as const;
