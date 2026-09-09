#!/usr/bin/env python3
"""Check the public release repository; optionally create only the absent repository."""
import argparse
import json
import os
import urllib.error
import urllib.request

BASE = 'https://hub.docker.com'
REPOSITORIES = '/v2/namespaces/opensourceops/repositories'
REPOSITORY = REPOSITORIES + '/agentctl'


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request(method, path, token=None, payload=None):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(BASE + path, method=method, headers=headers,
                                 data=json.dumps(payload).encode() if payload is not None else None)
    try:
        with urllib.request.build_opener(NoRedirect).open(req, timeout=30) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        # Never print authentication payloads, response bodies, or bearer tokens.
        return error.code, None
    except (urllib.error.URLError, TimeoutError, ValueError):
        raise RuntimeError('Docker Hub transport or response failed; inspect remote state before retrying') from None


def public_repository(value):
    if (not isinstance(value, dict) or value.get('namespace') != 'opensourceops'
            or value.get('name') != 'agentctl' or value.get('is_private') is not False):
        raise RuntimeError('Expected the public opensourceops/agentctl repository; existing visibility and access were not changed')


def check(username, secret, create=False):
    if not username or not secret:
        raise RuntimeError('Configure DOCKERHUB_USERNAME and DOCKERHUB_TOKEN in Actions')
    status, auth = request('POST', '/v2/auth/token', payload={'identifier': username, 'secret': secret})
    if (status != 200 or not isinstance(auth, dict)
            or not isinstance(auth.get('access_token'), str) or not auth['access_token']):
        raise RuntimeError(f'Docker Hub authentication failed (HTTP {status}); verify the publishing credential')
    token = auth['access_token']
    status, value = request('GET', REPOSITORY, token)
    created = False
    if status == 404 and create:
        status, value = request('POST', REPOSITORIES, token, {
            'name': 'agentctl', 'namespace': 'opensourceops', 'registry': 'docker.io',
            'is_private': False, 'description': 'Deterministic control plane for bounded, policy-constrained agent automation.'})
        if status != 201:
            raise RuntimeError(f'Docker Hub creation returned HTTP {status}; inspect the repository before retrying')
        created = True
    elif status != 200:
        raise RuntimeError(f'Docker Hub repository check returned HTTP {status}; create the missing public repository or correct access')
    public_repository(value)
    # Anonymous verification proves consumers can discover it without CI credentials.
    status, value = request('GET', REPOSITORY)
    if status != 200:
        raise RuntimeError(f'Public repository verification returned HTTP {status}')
    public_repository(value)
    return {'repository': 'docker.io/opensourceops/agentctl', 'public': True, 'created': created,
            'imagePushAccess': 'not_checked'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--create-if-missing', action='store_true')
    args = parser.parse_args()
    if (os.environ.get('GITHUB_REPOSITORY') != 'opensourceops/agentctl'
            or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'
            or os.environ.get('GITHUB_REF') != 'refs/heads/main'):
        raise SystemExit('Registry setup requires the trusted manual workflow on main')
    try:
        print(json.dumps(check(os.environ.get('DOCKERHUB_USERNAME'), os.environ.get('DOCKERHUB_TOKEN'), args.create_if_missing)))
    except RuntimeError as error:
        raise SystemExit(str(error)) from None


if __name__ == '__main__':
    main()
