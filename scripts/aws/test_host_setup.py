"""The shipped setup entry point with fake AWS/Wrangler on PATH; no cloud resources or real secrets.

The operator relies on keyless setup, ownership refusal and cleanup after an unconfirmed handoff.
The provider relies on published-key preservation and the policy's resource boundaries.
"""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import host_setup as setup

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]

# Fixture identifiers are synthesized; the fake persists key generations, never key values.
FAKE_AWS = r'''#!/usr/bin/env python3
import fnmatch, json, os, sys
from pathlib import Path
args = sys.argv[1:]
service, op, rest = args[6], args[7], args[8:]
p = Path(os.environ['WORLD'])
s = json.loads(p.read_text())
def opt(n): return rest[rest.index(n)+1]
def ident(n): return n + '-' + format(1, '017x')
def access(n): return 'AKIA' + str(n).zfill(16)
def out(v):
 p.write_text(json.dumps(s)); print(json.dumps(v)); sys.exit(0)
def fail(code):
 p.write_text(json.dumps(s)); print('An error occurred ('+code+') when calling operation', file=sys.stderr); sys.exit(254)
logged = list(args)
if '--access-key-id' in logged: logged[logged.index('--access-key-id')+1] = '<key-id>'
s['calls'].append({'args':logged,'inherited_credential':any(k in os.environ for k in ['AWS_ACCESS_KEY_ID','AWS_SECRET_ACCESS_KEY','CLOUDFLARE_GLOBAL_KEY','CLOUDFLARE_API_TOKEN','CLOUDFLARE_API_KEY','CF_API_TOKEN'])})
if os.environ.get('HISTORY'):
 print("FileExistsError: [Errno 17] File exists: '/dev/null'",file=sys.stderr);sys.exit(1)
if os.environ.get('FAIL_OP') == op: fail('AccessDenied')
if os.environ.get('MALFORMED_OP') == op: out({'AccountName':None})
a = str(10**11)
if op == 'get-caller-identity': out({'Account':a})
if op == 'get-account-information': out({'AccountName': 'Other' if os.environ.get('WRONG_ACCOUNT') else 'Overmatch'})
if op == 'describe-instance-types': out({'InstanceTypes':[{'VCpuInfo':{'DefaultVCpus':json.loads((Path(os.environ['REPO'])/'backend/production-server.json').read_text())['vcpu']}}]})
if op == 'describe-vpcs': out({'Vpcs':[{'VpcId':ident('vpc')}]})
if op == 'describe-subnets': out({'Subnets':[{'SubnetId':'subnet-'+format(n,'017x'),'State':'available','AvailabilityZone':'ap-southeast-1'+z} for n,z in enumerate('abc',1)]})
if op == 'describe-instance-type-offerings': out({'InstanceTypeOfferings':[{'InstanceType':opt('--filters').split('Values=')[1],'Location':'ap-southeast-1'+z} for z in os.environ.get('OFFER_ZONES','bc')]})
if op == 'get-parameter': out({'Parameter':{'Value':ident('ami'),'ARN':'arn:aws:ssm:ap-southeast-1::parameter'+opt('--name')}})
if op == 'describe-images': out({'Images':[{'ImageId':ident('ami'),'OwnerId':str(10**11+1),'Public':True,'State':'available','Architecture':'x86_64','Name':'ubuntu/images/hvm-ssd-gp3/ubuntu-resolute-26.04-amd64-server-1','RootDeviceName':'/dev/fixture','BlockDeviceMappings':[{'DeviceName':'/dev/fixture','Ebs':{'VolumeSize':8,'VolumeType':'gp3'}}]}]})
if op == 'describe-security-groups':
 if 'Name=group-name,Values=default' in rest: out({'SecurityGroups':[{'GroupId':ident('sg'),'VpcId':ident('vpc')}]})
 out({'SecurityGroups':s['groups']})
if op == 'create-security-group':
 s['groups'] = [{'GroupId':ident('sg'),'GroupName':opt('--group-name'),'Tags':json.loads(opt('--tag-specifications'))[0]['Tags'] if '--tag-specifications' in rest else [],'IpPermissions':[],'IpPermissionsEgress':[{'IpProtocol':'-1','IpRanges':[{'CidrIp':'0.0.0.0/0'}]}]}]; out({'GroupId':ident('sg')})
if op == 'authorize-security-group-ingress':
 s['groups'][0]['IpPermissions']=json.loads(opt('--ip-permissions')); out({})
if op == 'revoke-security-group-ingress': s['groups'][0]['IpPermissions']=[];out({})
if op == 'delete-security-group': s['groups']=[];out({})
if op == 'get-user':
 if not s['user']: fail('NoSuchEntity')
 out({'User':{'Arn':'arn:aws:iam::'+a+':user/overmatch/overmatch-host','Tags':s['user_tags'],'Path':s['user_path']}})
if op == 'create-user':
 s['user']=True;s['user_tags']=json.loads(opt('--tags')) if '--tags' in rest else [];s['user_path']=opt('--path') if '--path' in rest else '/';out({})
if op == 'delete-user': s['user']=False;out({})
if op == 'list-attached-user-policies': out({'AttachedPolicies':s.get('attached',[])})
if op == 'list-groups-for-user': out({'Groups':[]})
if op == 'list-user-policies': out({'PolicyNames':['overmatch-host'] if s['policy'] else []})
if op == 'get-login-profile': fail('NoSuchEntity')
if op == 'get-user-policy':
 if not s['policy']:fail('NoSuchEntity')
 out({'PolicyDocument':s['policy']})
if op == 'put-user-policy': s['policy']=json.loads(opt('--policy-document'));out({})
if op == 'delete-user-policy': s['policy']=None;out({})
if op == 'list-access-keys': out({'AccessKeyMetadata':[{'AccessKeyId':access(n),'CreateDate':'2026-01-01T00:00:'+str(n).zfill(2)+'Z','Status':'Active'} for n in s['keys']]})
if op == 'create-access-key':
 s['generation']+=1;n=s['generation'];s['keys'].append(n)
 if os.environ.get('AWS_CLI_HISTORY_FILE')!='/dev/null/host-setup-history-disabled':
  (p.parent/'aws-history').write_text('fixture-'+chr(73)*40+str(n))
 if os.environ.get('LOST_CREATE'):fail('RequestTimeout')
 out({'AccessKey':{'AccessKeyId':access(n),'SecretAccessKey':'fixture-'+chr(73)*40+str(n)}})
if op == 'delete-access-key':
 n=int(opt('--access-key-id')[4:])
 if os.environ.get('FAIL_OLD_REVOKE') and n==1:fail('AccessDenied')
 s['keys'].remove(n);s.setdefault('revoked',[]).append(n);out({})
if op in ['simulate-custom-policy','simulate-principal-policy']:
 policy=json.loads(opt('--policy-input-list')) if op=='simulate-custom-policy' else s['policy']
 ctx={c['ContextKeyName']:c['ContextKeyValues'][0] for c in json.loads(opt('--context-entries'))}
 action,resource=opt('--action-names'),opt('--resource-arns')
 def matches(st):
  acts=st['Action'] if isinstance(st['Action'],list) else [st['Action']]
  res=st['Resource'] if isinstance(st['Resource'],list) else [st['Resource']]
  return any(fnmatch.fnmatchcase(action,a) for a in acts) and any(fnmatch.fnmatchcase(resource,r) for r in res) and all(k in ctx and fnmatch.fnmatchcase(ctx[k],v) for k,v in st.get('Condition',{}).get('StringLike',{}).items()) and all(k in ctx and float(ctx[k])<=v for k,v in st.get('Condition',{}).get('NumericLessThanEquals',{}).items())
 matching=[st for st in policy['Statement'] if matches(st)]
 allow=any(st['Effect']=='Allow' for st in matching) and not any(st['Effect']=='Deny' for st in matching)
 out({'EvaluationResults':[{'EvalDecision':json.loads(os.environ['DECISION']) if 'DECISION' in os.environ and not allow else ('allowed' if allow else 'implicitDeny')}]})
if op == 'run-instances':
 if '--dry-run' not in rest: fail('UnsafeTest')
 fail('DryRunOperation')
if op == 'describe-instances':out({'Reservations':[{'Instances':s.get('instances',[])}]})
if op in ['describe-volumes','describe-network-interfaces','describe-key-pairs']:
 out({{'describe-volumes':'Volumes','describe-network-interfaces':'NetworkInterfaces','describe-key-pairs':'KeyPairs'}[op]:[]})
fail('UnhandledFakeOperation')
'''

FAKE_WRANGLER = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
p=Path(os.environ['WORLD']);s=json.loads(p.read_text());args=sys.argv[1:]
if args==['--version']:
 if os.environ.get('WRANGLER_LOG')=='log':print(json.loads((Path(os.environ['REPO'])/'backend/package.json').read_text())['devDependencies']['wrangler'])
 sys.exit(0)
s['worker_calls'].append({'args':args,'cwd':os.getcwd()})
if args[:2]==['secret','list']:
 if os.environ.get('WRANGLER_LOG')=='log':print(json.dumps([{'name':n} for n in s['secrets']]))
elif args[:2]==['secret','delete']:
 s['secrets'].remove(args[2]);print('deleted')
else:
 value=sys.stdin.read().strip()
 if os.environ.get('WRANGLER_WRITE_LOGS')!='false':(p.parent/'wrangler-debug.log').write_text(value)
 # A CLI which echoes even on failure must not expose the pipe to the operator.
 print(value);print(value,file=sys.stderr)
 n=s['generation']
 if args[2]=='AWS_ACCESS_KEY_ID': good=value=='AKIA'+str(n).zfill(16)
 elif args[2]=='AWS_SECRET_ACCESS_KEY':good=value=='fixture-'+chr(73)*40+str(n)
 else:
  config=json.loads(value);good=set(config)=={'subnetIds','securityGroupId'} and len(config['subnetIds'])==2
 s['handoffs'].append({'name':args[2],'correct':good,'keys':list(s['keys'])})
 if args[2] not in s['secrets']:s['secrets'].append(args[2])
p.write_text(json.dumps(s))
if os.environ.get('FAIL_SECRET')==args[2] if len(args)>2 else False:sys.exit(1)
'''


class Fixture:
    def __init__(self, testcase, **changes):
        self.temp = tempfile.TemporaryDirectory()
        testcase.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.world_path = self.root / 'world.json'
        self.state = dict(calls=[], worker_calls=[], user=False, groups=[], policy=None, keys=[],
                          generation=0, secrets=[], handoffs=[])
        self.state.update(changes)
        self.save()
        for name, text in [('aws', FAKE_AWS), ('wrangler', FAKE_WRANGLER)]:
            path = self.root / name
            path.write_text(text)
            path.chmod(0o700)
        self.env = os.environ | {'PATH': str(self.root) + os.pathsep + os.environ['PATH'],
                                 'WORLD': str(self.world_path), 'REPO': str(ROOT),
                                 'AWS_ACCESS_KEY_ID': 'parent-must-not-be-used',
                                 'AWS_SECRET_ACCESS_KEY': 'parent-must-not-be-used',
                                 'CLOUDFLARE_API_TOKEN': 'parent-must-not-be-used',
                                 'CLOUDFLARE_API_KEY': 'parent-must-not-be-used',
                                 'CF_API_TOKEN': 'parent-must-not-be-used'}
        self.output = ''

    def save(self):
        self.world_path.write_text(json.dumps(self.state))

    def run(self, *args, **env):
        result = subprocess.run(['sh', '-x', str(HERE / 'host-setup'), *args], capture_output=True,
                                text=True, env=self.env | env, timeout=30)
        self.state = json.loads(self.world_path.read_text())
        self.output += result.stdout + result.stderr
        return result

    def ops(self):
        return [c['args'][7] for c in self.state['calls']]


class Setup(unittest.TestCase):
    def ready(self):
        f = Fixture(self)
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        return f

    def issued(self):
        f = self.ready()
        self.assertEqual(f.run('issue-key', '--env', 'aws-proving').returncode, 0, f.output)
        return f

    def test_real_pinned_wrangler_version_is_read_under_the_shipped_environment(self):
        # Guards pinned Logger behaviour: error-level suppression must never masquerade as a version mismatch.
        binary = ROOT / 'backend/node_modules/.bin/wrangler'
        self.assertTrue(binary.exists(), 'install backend dependencies with the documented npm ci command')
        result = setup.command([str(binary), '--version'])
        self.assertEqual(result.returncode, 0)
        expected = json.loads((ROOT / 'backend/package.json').read_text())['devDependencies']['wrangler']
        self.assertEqual(result.stdout.strip(), expected)
        script = "const {unstable_readConfig:read}=require('wrangler');console.log(JSON.stringify(['','aws-proving'].map(env=>read({config:'wrangler.jsonc',env},{hideWarnings:true}).name)))"
        config = setup.command(['node', '-e', script], cwd=ROOT / 'backend')
        self.assertEqual(config.returncode, 0)
        self.assertEqual(json.loads(config.stdout), ['overmatch-backend', 'overmatch-backend-aws-proving'])

    def test_plan_and_bad_arguments_cannot_touch_the_account(self):
        # The review command and typos must never cross the AWS/Worker boundary.
        f = Fixture(self)
        self.assertEqual(f.run('plan').returncode, 0)
        for args in [[], ['setup'], ['apply', '--region', 'elsewhere'], ['apply', 'extra'], ['issue-key'], ['rotate-key'], ['teardown'], ['issue-key', '--env', 'typo']]:
            self.assertEqual(f.run(*args).returncode, 2)
        self.assertEqual(f.state['calls'], [])
        self.assertEqual(f.state['worker_calls'], [])

    def test_apply_is_restartable_and_never_creates_a_key_or_calls_wrangler(self):
        f = Fixture(self)
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        first = len(f.state['calls'])
        # AWS describes extra empty fields; they cannot cause a new ingress rule every run.
        rule = f.state['groups'][0]['IpPermissions'][0]
        rule.update(Ipv6Ranges=[], PrefixListIds=[], UserIdGroupPairs=[])
        rule['IpRanges'][0]['Description'] = 'game'
        f.save()
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        ops = [c['args'][7] for c in f.state['calls'][first:]]
        for op in ['create-user', 'create-security-group', 'put-user-policy', 'create-access-key',
                   'revoke-security-group-ingress', 'authorize-security-group-ingress']:
            self.assertNotIn(op, ops)
        self.assertEqual(f.state['keys'], [])
        self.assertEqual(f.state['worker_calls'], [])
        self.assertEqual(f.state['handoffs'], [])
        for call in f.state['calls']:
            self.assertEqual(call['args'][:4], ['--profile', 'overmatch', '--region', 'ap-southeast-1'])
            self.assertFalse(call['inherited_credential'])
        self.assertTrue(all(Path(c['cwd']) == ROOT / 'backend' for c in f.state['worker_calls']))

    def test_drift_is_closed_and_ingress_tracks_the_server_owner(self):
        f = Fixture(self)
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        f.state['groups'][0]['IpPermissions'] = [{'IpProtocol': '-1'}]
        f.save()
        start = len(f.state['calls'])
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        calls = f.state['calls'][start:]
        changed = [c for c in calls if c['args'][7] in ['authorize-security-group-ingress', 'revoke-security-group-ingress']]
        self.assertEqual([c['args'][7] for c in changed], ['revoke-security-group-ingress', 'authorize-security-group-ingress'])
        rules = f.state['groups'][0]['IpPermissions']
        import re
        port = int(re.search(r'^const PORT: u16 = ([0-9]+);$', (ROOT / 'src/net/server.rs').read_text(), re.M)[1])
        self.assertEqual([(r['IpProtocol'], r['FromPort'], r['ToPort']) for r in rules], [('udp', port, port)])
        self.assertEqual(rules[0]['IpRanges'], [{'CidrIp': '0.0.0.0/0'}])

    def test_wrong_account_and_foreign_ownership_refuse_before_mutation(self):
        f = Fixture(self)
        self.assertEqual(f.run('apply', WRONG_ACCOUNT='1').returncode, 1)
        self.assertIn('wrong AWS account', f.output)
        self.assertNotIn('create-user', f.ops())
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        saved_tags = f.state['user_tags']
        for change, cause in [({'user_tags': []}, 'ownership'), ({'user_path': '/'}, 'path'), ({'attached': [{'name': 'foreign'}]}, 'extra permissions')]:
            f.state.update(change)
            f.save()
            start = len(f.state['calls'])
            result = f.run('apply')
            self.assertEqual(result.returncode, 1)
            self.assertIn(cause, result.stderr)
            self.assertFalse(any(c['args'][7].startswith(('create-', 'put-', 'revoke-', 'authorize-')) for c in f.state['calls'][start:]))
            f.state['user_tags'] = saved_tags
            f.state['user_path'] = '/overmatch/'
            f.state.pop('attached', None)

    def test_the_key_pipeline_never_writes_or_prints_credentials_even_if_wrangler_echoes(self):
        for failed in ['', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY']:
            with self.subTest(failed=failed):
                f = self.ready()
                result = f.run('issue-key', '--env', 'aws-proving', FAIL_SECRET=failed)
                self.assertEqual(result.returncode, 1 if failed else 0, f.output)
                secret = 'fixture-' + chr(73) * 40 + '1'
                access = 'AKIA' + str(1).zfill(16)
                for value in (secret, access):
                    self.assertNotIn(value, f.output)
                    for path in f.root.iterdir():
                        self.assertNotIn(value, path.read_text(), path.name)
                self.assertEqual(f.state['keys'], [] if failed else [1])
                if failed:
                    self.assertEqual(f.state['revoked'], [1])
                self.assertNotIn('Traceback', f.output)

    def test_rotation_keeps_the_old_key_until_both_secrets_are_confirmed(self):
        f = self.issued()
        self.assertEqual(f.run('rotate-key', '--env', 'aws-proving', FAIL_SECRET='AWS_SECRET_ACCESS_KEY').returncode, 1)
        self.assertEqual(f.state['keys'], [1])
        self.assertEqual(f.state['revoked'], [2])
        self.assertEqual(f.run('rotate-key', '--env', 'aws-proving').returncode, 0, f.output)
        self.assertEqual(f.state['keys'], [3])
        self.assertEqual(f.state['revoked'], [2, 1])
        last_put = max(i for i, c in enumerate(f.state['handoffs']) if c['name'] == 'AWS_SECRET_ACCESS_KEY')
        self.assertTrue(f.state['handoffs'][last_put]['correct'])
        self.assertEqual(f.state['handoffs'][last_put]['keys'], [1, 3])

    def test_failed_old_key_deletion_keeps_the_key_the_worker_now_holds(self):
        f = self.issued()
        self.assertEqual(f.run('rotate-key', '--env', 'aws-proving', FAIL_OLD_REVOKE='1').returncode, 1)
        self.assertEqual(f.state['keys'], [1, 2])
        self.assertNotIn('revoked', f.state)
        self.assertIn('OldKeyDeletionFailed', f.output)
        self.assertIn('2026-01-01T00:00:01Z', f.output)
        self.assertIn('rerun', f.output)

    def test_lost_creation_response_is_reconciled_and_the_unpublished_key_revoked(self):
        f = self.ready()
        self.assertEqual(f.run('issue-key', '--env', 'aws-proving', LOST_CREATE='1').returncode, 1)
        self.assertEqual(f.state['keys'], [])
        self.assertEqual(f.state['revoked'], [1])
        self.assertEqual(f.run('issue-key', '--env', 'aws-proving').returncode, 0, f.output)
        self.assertEqual(f.state['keys'], [2])

    def test_cli_failures_are_named_without_echoing_aws_diagnostics_or_a_traceback(self):
        f = Fixture(self)
        result = f.run('apply', FAIL_OP='describe-vpcs')
        self.assertEqual(result.returncode, 1)
        self.assertIn('AccessDenied', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertNotIn('create-user', f.ops())
        malformed = f.run('apply', MALFORMED_OP='get-account-information')
        self.assertEqual(malformed.returncode, 1)
        self.assertIn('malformed', malformed.stderr)
        self.assertNotIn('Traceback', malformed.stderr)

    def test_verify_checks_installed_policy_and_rules_without_a_key(self):
        f = self.ready()
        self.assertEqual(f.run('verify').returncode, 0, f.output)
        self.assertIn('simulate-principal-policy', f.ops())
        self.assertNotIn('create-access-key', f.ops())
        self.assertNotIn('run-instances', f.ops())
        self.assertEqual(f.state['worker_calls'], [])
        f.state['policy']['Statement'].append({'Effect': 'Allow', 'Action': 'iam:*', 'Resource': '*'})
        f.save()
        self.assertIn('policy has drifted', f.run('verify').stderr)
        self.assertEqual(f.run('apply').returncode, 0, f.output)
        for field in ['IpPermissions', 'IpPermissionsEgress']:
            saved = f.state['groups'][0][field]
            f.state['groups'][0][field] = []
            f.save()
            self.assertIn('has drifted', f.run('verify').stderr)
            f.state['groups'][0][field] = saved
            f.save()

    def test_unknown_simulator_decisions_cannot_be_counted_as_denial(self):
        f = self.ready()
        for decision in [None, 'unknown', {}, 'explicitDeny']:
            result = f.run('verify', DECISION=json.dumps(decision))
            self.assertEqual(result.returncode, 0 if decision == 'explicitDeny' else 1)
            if decision != 'explicitDeny':
                self.assertIn('unknown decision', result.stderr)

    def test_issue_refuses_existing_keys_and_rotation_requires_one(self):
        f = self.ready()
        self.assertEqual(f.run('rotate-key', '--env', 'production').returncode, 1)
        self.assertNotIn('create-access-key', f.ops())
        self.assertEqual(f.run('issue-key', '--env', 'production').returncode, 0, f.output)
        self.assertEqual(f.run('issue-key', '--env', 'aws-proving').returncode, 1)
        self.assertEqual(f.state['keys'], [1])
        self.assertTrue(all(h['correct'] for h in f.state['handoffs']))
        self.assertEqual({h['name'] for h in f.state['handoffs']}, set(setup.SECRETS))
        puts = [c for c in f.state['worker_calls'] if c['args'][1] == 'put']
        self.assertTrue(all(c['args'][-2:] == ['--env', ''] for c in puts))

    def test_offering_subnets_are_all_handed_to_the_provider(self):
        f = self.issued()
        policy = f.state['policy']
        network = policy['Statement'][1]['Resource']
        self.assertTrue(any(r.endswith('subnet/subnet-' + format(2,'017x')) for r in network))
        self.assertTrue(any(r.endswith('subnet/subnet-' + format(3,'017x')) for r in network))
        self.assertFalse(any(r.endswith('subnet/subnet-' + format(1,'017x')) for r in network))
        self.assertTrue(all(c['args'][-2:] == ['--env', 'aws-proving'] for c in f.state['worker_calls']))
        empty = Fixture(self)
        self.assertEqual(empty.run('apply', OFFER_ZONES='').returncode, 1)
        self.assertNotIn('create-user', empty.ops())
        all_zones = Fixture(self)
        self.assertEqual(all_zones.run('apply', OFFER_ZONES='abc').returncode, 0, all_zones.output)

    def test_invalid_policy_templates_refuse_before_the_first_mutation(self):
        # The operator must not get partial standing resources from a local template fault.
        for template in ['{', '{"Statement":[{"Resource":"@TYPO@"}]}', '{"Statement":null}']:
            f = Fixture(self)
            for rel in ['backend/production-server.json', 'src/net/server.rs']:
                dest = f.root / rel
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text((ROOT / rel).read_text())
            policy = f.root / 'scripts/aws/host-policy.json'
            policy.parent.mkdir(parents=True)
            policy.write_text(template)
            with mock.patch.object(setup, 'ROOT', f.root), mock.patch.dict(os.environ, f.env), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(setup.main(['apply']), 1)
            state = json.loads(f.world_path.read_text())
            self.assertFalse(any(c['args'][7].startswith(('create-', 'put-', 'authorize-', 'revoke-')) for c in state['calls']))

    def test_history_refusal_names_the_operator_fix(self):
        f = Fixture(self)
        self.assertIn('disable cli_history', f.run('apply', HISTORY='1').stderr)
        self.assertNotIn('create-user', f.ops())

    def test_teardown_is_restartable_and_refuses_to_orphan_running_hosts(self):
        f = self.issued()
        f.state['instances'] = [{'State': {'Name': 'running'}}]
        f.save()
        result = f.run('teardown', '--env', 'aws-proving')
        self.assertEqual(result.returncode, 1)
        self.assertIn('drain', result.stderr)
        self.assertEqual(f.state['keys'], [1])
        f.state['instances'] = []
        f.save()
        self.assertEqual(f.run('teardown', '--env', 'aws-proving').returncode, 0, f.output)
        self.assertFalse(f.state['user'])
        self.assertEqual((f.state['groups'], f.state['keys'], f.state['secrets']), ([], [], []))
        self.assertEqual(f.run('teardown', '--env', 'aws-proving').returncode, 0, f.output)
        self.assertIn('Teardown proof:', f.output)


if __name__ == '__main__':
    unittest.main()
