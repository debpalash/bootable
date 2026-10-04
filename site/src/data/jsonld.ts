const SITE = 'https://bootable.palash.dev';

/** SoftwareApplication entry for Bootable. Only facts stated on the site. */
export function bootableApp() {
  return {
    '@context': 'https://schema.org',
    '@type': 'SoftwareApplication',
    name: 'Bootable',
    url: SITE + '/',
    applicationCategory: 'UtilitiesApplication',
    operatingSystem: 'Linux, Windows 10+, macOS (Apple Silicon)',
    softwareVersion: '0.1.1',
    license: 'https://www.apache.org/licenses/LICENSE-2.0',
    isAccessibleForFree: true,
    downloadUrl: SITE + '/download',
    codeRepository: 'https://github.com/debpalash/bootable',
    description:
      'Open-source USB and SD image writer for Linux, Windows, and macOS with read-back verification and removable-drive-only safety checks.',
    offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
  };
}

export function breadcrumbs(items: { name: string; path?: string }[]) {
  return {
    '@context': 'https://schema.org',
    '@type': 'BreadcrumbList',
    itemListElement: items.map((item, i) => ({
      '@type': 'ListItem',
      position: i + 1,
      name: item.name,
      ...(item.path !== undefined ? { item: SITE + item.path } : {}),
    })),
  };
}

export function webPage(name: string, path: string, description: string, dateModified: string) {
  return {
    '@context': 'https://schema.org',
    '@type': 'WebPage',
    name,
    url: SITE + path,
    description,
    dateModified,
    isPartOf: { '@type': 'WebSite', name: 'Bootable', url: SITE + '/' },
  };
}
