const UPSTREAM_ORIGIN = 'https://debpalash.github.io';
const UPSTREAM_PREFIX = '/bootable';

export default {
  async fetch(request) {
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      return new Response('Method not allowed', {
        status: 405,
        headers: { Allow: 'GET, HEAD' },
      });
    }

    const incoming = new URL(request.url);
    const upstream = new URL(`${UPSTREAM_PREFIX}${incoming.pathname}`, UPSTREAM_ORIGIN);
    upstream.search = incoming.search;

    const upstreamResponse = await fetch(upstream, {
      method: request.method,
      headers: request.headers,
      redirect: 'manual',
    });
    const response = new Response(upstreamResponse.body, upstreamResponse);
    response.headers.set('Content-Security-Policy', [
      "default-src 'self'",
      "base-uri 'none'",
      "form-action 'none'",
      "frame-ancestors 'none'",
      "img-src 'self' data:",
      "object-src 'none'",
      "script-src 'none'",
      "style-src 'self'",
    ].join('; '));
    response.headers.set('Permissions-Policy', 'camera=(), geolocation=(), microphone=()');
    response.headers.set('Referrer-Policy', 'strict-origin-when-cross-origin');
    response.headers.set('X-Content-Type-Options', 'nosniff');
    response.headers.set('X-Frame-Options', 'DENY');
    return response;
  },
};
