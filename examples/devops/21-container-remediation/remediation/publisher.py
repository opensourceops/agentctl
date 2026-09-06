#!/usr/bin/env python3
"""Trusted, separate publisher: exact tree, fixed repository, stable PR identity."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import adapter
from runner import digest_file, git as base_git, json_file, save


def git_environment(values=None):
    env = dict(os.environ if values is None else values)
    for key in list(env):
        if key.startswith('GIT_CONFIG') or key in {'GIT_ASKPASS', 'SSH_ASKPASS', 'GIT_SSH', 'GIT_SSH_COMMAND'}:
            env.pop(key)
    env.update({'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_SYSTEM': '/dev/null', 'GIT_CONFIG_GLOBAL': '/dev/null',
                'GIT_TERMINAL_PROMPT': '0'})
    return env


def git(*args, cwd, env=None, data=None):
    return base_git('-c', 'credential.helper=', '-c', 'http.extraHeader=', *args, cwd=cwd, env=git_environment(env), data=data)


def publication_runs(bundle, decision):
    runs = json_file(Path(bundle)/'runs.json')
    invocation = runs.get('invocation', {})
    policy = adapter.contract()
    if invocation.get('repository') != policy['repository'] or invocation.get('sourceSha') != decision['sourceSha']:
        raise ValueError('run evidence belongs to another repository or source')
    if invocation.get('workflowRef') != policy['repository']+'/.github/workflows/remediation.yml@refs/heads/main':
        raise ValueError('run evidence belongs to another workflow')
    if any(not re.fullmatch(r'[1-9][0-9]*', str(invocation.get(key, ''))) for key in ['runId', 'runAttempt']):
        raise ValueError('original CI run and attempt evidence are missing')
    for name in ['remediation', 'eligibility']:
        if not isinstance(runs.get(name), dict) or not re.fullmatch(r'run-[0-9a-f-]+', runs[name].get('runId', '')):
            raise ValueError('both actual agentctl run identities are required')
    if runs['remediation'].get('providerRequests', 0) < 2 or runs['remediation'].get('toolCalls') != 2 or runs['eligibility'].get('providerRequests') != 0:
        raise ValueError('run evidence does not establish two-role live execution and credential-free eligibility')
    return runs


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise urllib.error.HTTPError(request.full_url, code, 'redirect rejected', headers, fp)


class GitHub:
    def __init__(self, repository, token):
        if repository != adapter.contract()['repository'] or not token:
            raise ValueError('publisher requires the configured demo scope and GH_TOKEN')
        self.repository, self.token = repository, token
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, method, path, body=None):
        url = 'https://api.github.com/repos/' + self.repository + '/' + path
        request = urllib.request.Request(url, method=method, data=adapter.canonical(body) if body is not None else None,
            headers={'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
                     'X-GitHub-Api-Version': '2022-11-28', 'Content-Type': 'application/json', 'User-Agent': 'agentctl-remediation-example'})
        try:
            with self.opener.open(request, timeout=30) as response:
                raw = response.read(adapter.MAX_BYTES + 1)
            return adapter.parse(raw)
        except urllib.error.HTTPError as error:
            if error.code == 404 and method == 'GET': return None
            raise RuntimeError('GitHub request failed with HTTP ' + str(error.code)) from None
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            raise RuntimeError('GitHub request acknowledgement is uncertain; reconcile with reads') from None

    def ref(self, name):
        return self.request('GET', 'git/ref/heads/' + urllib.parse.quote(name, safe='/'))

    def branch_tree(self, branch):
        ref = self.ref(branch)
        if ref is None: return None
        commit = self.request('GET', 'git/commits/' + ref['object']['sha'])
        return commit['tree']['sha']

    def pulls(self, branch, base):
        query = urllib.parse.urlencode({'state': 'all', 'head': self.repository.split('/')[0]+':'+branch, 'base': base, 'per_page': 100})
        result = self.request('GET', 'pulls?' + query)
        if not isinstance(result, list) or len(result) >= 100:
            raise ValueError('PR reconciliation inventory is invalid or truncated')
        return result


def verify_bundle(bundle, checkout):
    bundle, checkout = Path(bundle).resolve(), Path(checkout).resolve()
    sums = json_file(bundle/'SHA256SUMS.json')
    present = {p.relative_to(bundle).as_posix() for p in bundle.rglob('*') if p.is_file() or p.is_symlink()}
    if set(sums) | {'SHA256SUMS.json'} != present:
        raise ValueError('publication artifact has missing or additional files')
    for name, expected in sums.items():
        candidate = bundle/name
        if Path(name).is_absolute() or '..' in Path(name).parts or candidate.is_symlink() or digest_file(candidate) != expected:
            raise ValueError('publication artifact path or digest mismatch')
    decision = json_file(bundle/'publication.json')
    if decision.get('eligible') is not True:
        return decision
    policy = adapter.contract()
    if git('rev-parse', 'HEAD', cwd=checkout) != decision['sourceSha'] or git('status', '--porcelain', cwd=checkout):
        raise ValueError('publisher must start from the clean exact validated source commit')
    if git('rev-parse', 'HEAD^{tree}', cwd=checkout) != decision['sourceTree']:
        raise ValueError('publisher source tree differs')
    if set(decision['files']) != set(policy['allowedFiles']):
        raise ValueError('publication widened the allowed patch paths')
    expected_files = adapter.expected_files(policy['targetVersion'])
    for name, expected in expected_files.items():
        if (bundle/'patch'/name).read_bytes() != expected or adapter.sha(expected) != decision['files'][name]:
            raise ValueError('publisher refused changed dependency bytes')
    # Re-evaluate the same deterministic eligibility contract with the fresh
    # trusted helper. The artifact cannot establish authority by claiming true.
    with tempfile.TemporaryDirectory(prefix='agentctl-publisher-verify-') as temporary:
        root = Path(temporary).resolve()
        for name in ['state', 'inputs', 'patch']:
            (root/name).mkdir()
        for original, target in [('context.json', 'state/context.json'), ('patch-ready.json', 'state/patch-ready.json'),
                                 ('before.json', 'inputs/before.json'), ('after.json', 'inputs/after.json'), ('gates.json', 'inputs/gates.json')]:
            shutil.copyfile(bundle/original, root/target)
        for name in policy['allowedFiles']:
            shutil.copyfile(bundle/'patch'/name, root/'patch'/name)
        previous = adapter.ROOT
        try:
            adapter.ROOT = root
            if adapter.eligibility({}) != decision:
                raise ValueError('publication decision differs from recomputed evidence')
        finally:
            adapter.ROOT = previous
    fingerprint = adapter.sha(adapter.canonical({'repository': policy['repository'], 'sourceSha': decision['sourceSha'], 'patchDigest': decision['patchDigest']}))
    if decision['fingerprint'] != fingerprint:
        raise ValueError('publication identity is not source/patch derived')
    publication_runs(bundle, decision)
    return decision


def checked_branch(decision, policy):
    adapter.require_digest(decision['fingerprint'])
    branch = policy['branchPrefix'] + decision['fingerprint'][:24]
    if not branch.startswith('agentctl/remediation/') or not re.fullmatch(r'agentctl/remediation/[0-9a-f]{24}', branch):
        raise ValueError('publication branch is outside the fixed prefix')
    return branch


def reconcile_pull(api, branch, base, body):
    existing = api.pulls(branch, base)
    if len(existing) > 1:
        raise ValueError('multiple matching PRs require manual reconciliation')
    if existing:
        return {'status': 'reused' if existing[0]['state'] == 'open' else 'closed_no_duplicate', 'pullRequest': existing[0]['html_url']}
    try:
        created = api.request('POST', 'pulls', body)
        return {'status': 'created', 'pullRequest': created['html_url']}
    except RuntimeError:
        # One read-only reconciliation after uncertain POST. Never repeat POST.
        existing = api.pulls(branch, base)
        if len(existing) == 1:
            return {'status': 'reconciled_after_uncertain_create', 'pullRequest': existing[0]['html_url']}
        raise RuntimeError('PR creation remains uncertain; retain evidence and reconcile this exact branch before any retry') from None


def publish(bundle, checkout, output):
    policy = adapter.contract()
    decision = verify_bundle(bundle, checkout)
    if not decision.get('eligible'):
        save(output, {'status': 'no_publication', 'decision': decision}); return
    if os.environ.get('GITHUB_REPOSITORY') != policy['repository']:
        raise ValueError('publisher job repository does not match the configured demo')
    if git('remote', 'get-url', 'origin', cwd=checkout) not in ['https://github.com/'+policy['repository'], 'https://github.com/'+policy['repository']+'.git']:
        raise ValueError('publisher Git remote differs from the explicit repository')
    api = GitHub(policy['repository'], os.environ.get('GH_TOKEN'))
    base = policy['baseBranch']; branch = checked_branch(decision, policy)
    if api.ref(base)['object']['sha'] != decision['sourceSha']:
        raise ValueError('default branch moved; rebuild and rescan the new source instead of rebasing this result')
    existing_tree = api.branch_tree(branch)
    if existing_tree is not None and existing_tree != decision['validatedTree']:
        raise ValueError('existing remediation branch has a different tree; never overwrite it')
    with tempfile.TemporaryDirectory(prefix='agentctl-publish-') as temporary:
        candidate = Path(temporary)/'candidate'
        git('worktree', 'add', '--detach', str(candidate), decision['sourceSha'], cwd=checkout)
        try:
            for name in policy['allowedFiles']:
                (candidate/name).write_bytes((Path(bundle)/'patch'/name).read_bytes())
            git('add', '--', *policy['allowedFiles'], cwd=candidate)
            if git('write-tree', cwd=candidate) != decision['validatedTree']:
                raise ValueError('publisher cannot reproduce the exact tested Git tree')
            if existing_tree is None:
                env = {**git_environment(), 'GIT_AUTHOR_NAME': 'agentctl remediation', 'GIT_AUTHOR_EMAIL': 'agentctl@example.invalid',
                       'GIT_COMMITTER_NAME': 'agentctl remediation', 'GIT_COMMITTER_EMAIL': 'agentctl@example.invalid'}
                commit = git('commit-tree', decision['validatedTree'], '-p', decision['sourceSha'], cwd=candidate, env=env,
                             data=b'fix: update the reviewed urllib3 dependency\n')
                askpass = Path(temporary)/'askpass.py'
                askpass.write_text('#!/usr/bin/env python3\nimport os,sys\nprint("x-access-token" if "Username" in sys.argv[1] else os.environ["GH_TOKEN"])\n')
                askpass.chmod(0o700)
                env.update({'GIT_ASKPASS': str(askpass), 'GIT_TERMINAL_PROMPT': '0'})
                # No force. A racing branch with different history cannot be overwritten.
                try:
                    result = subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'credential.helper=', '-c', 'http.extraHeader=', 'push', '--porcelain', 'origin', commit+':refs/heads/'+branch],
                                            cwd=candidate, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
                    uncertain = result.returncode != 0
                except subprocess.TimeoutExpired:
                    uncertain = True
                if uncertain and api.branch_tree(branch) != decision['validatedTree']:
                    raise RuntimeError('branch push failed or remains uncertain; no PR request was sent')
        finally:
            git('worktree', 'remove', '--force', str(candidate), cwd=checkout)
    if api.branch_tree(branch) != decision['validatedTree'] or api.ref(base)['object']['sha'] != decision['sourceSha']:
        raise ValueError('remote source/tree changed before publication')
    runs = publication_runs(bundle, decision)
    context = json_file(Path(bundle)/'context.json')
    workflow_link = 'https://github.com/'+policy['repository']+'/actions/runs/'+runs['invocation']['runId']+'/attempts/'+runs['invocation']['runAttempt']
    body = {'title': 'fix: remediate scanned urllib3 vulnerabilities', 'head': branch, 'base': base, 'draft': True,
            'body': 'Updates urllib3 from 2.6.2 to the reviewed 2.7.0 wheel.\n\n'
                    'Application tests, an actual image rebuild and a rescan with the same captured Trivy database passed. '
                    'The targeted advisories are absent; unrelated residual findings remain explicit in the workflow artifact.\n\n'
                    f'Source: `{decision["sourceSha"]}`\nValidated tree: `{decision["validatedTree"]}`\n'
                    f'Patch fingerprint: `{decision["fingerprint"]}`\nBefore image: `{decision["beforeImageId"]}`\n'
                    f'After image: `{decision["afterImageId"]}`\nDatabase SHA256: `{decision["databaseSha256"]}`\n\n'
                    'Two bounded Astra roles staged the patch. Build, scan and publication are trusted outer CI effects. '
                    'No release or production image was published.\n\n'
                    f'Agentctl remediation run: `{runs["remediation"]["runId"]}`\nEligibility run: `{runs["eligibility"]["runId"]}`\n'
                    f'[Original CI execution and retained evidence]({workflow_link})\n\n'
                    '| Finding scope | Before | After |\n| --- | ---: | ---: |\n'
                    f'| Target urllib3 advisories | {len(context["targets"])} | 0 |\n'
                    f'| Unrelated residual findings | {context["residualCount"]} | {len(decision["residualFindings"])} |\n'
                    '| New HIGH/CRITICAL findings | — | 0 |\n'}
    result = reconcile_pull(api, branch, base, body)
    save(output, {**result, 'branch': branch, 'sourceSha': decision['sourceSha'], 'validatedTree': decision['validatedTree'], 'fingerprint': decision['fingerprint']})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, required=True)
    parser.add_argument('--checkout', type=Path, default=Path.cwd())
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(); publish(args.bundle, args.checkout, args.output)

if __name__ == '__main__':
    try: main()
    except Exception as error:
        print(type(error).__name__+': '+str(error), file=__import__('sys').stderr)
        __import__('sys').exit(1)
