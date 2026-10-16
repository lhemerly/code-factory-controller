"""Lifecycle and in-flight accounting regressions from the live scope review."""
import copy
import datetime as dt
import unittest
from dashboard import domain, loader, render
from dashboard.tests import support

class CurrentStateTest(unittest.TestCase):
    def raw(self):
        return loader.load_state(support.FIXTURES / 'mixed')

    def test_accepted_one_cycle_awaits_review_despite_fresher_waiting_status(self):
        raw = self.raw()
        goal = raw['projects'][2]
        goal['status'].update(status='waiting', updated_utc='2026-01-01T11:59:00Z')
        goal['dispatcher_goal'].update(state='paused', native_status='accepted')
        raw['dispatcher']['updated_utc'] = '2026-01-01T09:00:00Z'
        dash = domain.build_dashboard(raw)
        track = dash['projects'][2]
        self.assertEqual(track['activity']['state'], 'awaiting_review')
        self.assertNotIn(track['goal_id'], dash['topbar']['queued_projects'])
        self.assertIn('awaiting review', render.render_html(dash))
        self.assertIn('awaiting review', track['task']['text'])

    def test_paused_completed_and_unknown_are_not_queued(self):
        for state, native, expected in [('paused','failed','paused'), ('complete','accepted','complete'), (None,None,'unknown')]:
            with self.subTest(state=state):
                raw = self.raw()
                goal = raw['projects'][2]
                goal['status'].update(status='waiting', updated_utc='2026-01-01T11:59:00Z')
                goal['dispatcher_goal'].clear()
                if state: goal['dispatcher_goal'].update(state=state,native_status=native)
                dash = domain.build_dashboard(raw)
                self.assertEqual(dash['projects'][2]['activity']['state'], expected)
                self.assertNotIn(goal['goal_id'], dash['topbar']['queued_projects'])

    def test_fresh_running_status_supersedes_older_pause(self):
        raw = self.raw()
        goal = raw['projects'][2]
        goal['status'].update(status='running', updated_utc='2026-01-01T11:59:00Z')
        goal['dispatcher_goal'].update(state='paused',native_status='accepted')
        dash = domain.build_dashboard(raw)
        self.assertEqual(dash['projects'][2]['activity']['state'],'active')

    def test_inflight_task_comes_from_exact_current_cycle(self):
        raw = self.raw()
        goal = raw['projects'][0]
        goal['status']['sequence'] = 2
        goal['cycles'].append({'dir':'000002','files':{'result.json':{'sequence':2,'status':'running','task':'Inspect the current contour gap'}}})
        track = domain.build_dashboard(raw)['projects'][0]
        self.assertIn('Inspect the current contour gap',track['task']['text'])
        self.assertNotIn('not present',track['task']['note'])

    def test_fresh_project_task_does_not_use_another_goals_old_sequence(self):
        raw = self.raw()
        goal = raw['projects'][2]
        goal['status'].update(status='running', sequence=9, current={'sequence':9}, updated_utc='2026-01-01T11:59:00Z')
        goal['cycles'].append({'dir':'000009','files':{'result.json':{'sequence':9,'status':'running','task':'Inspect this project'}}})
        track = domain.build_dashboard(raw)['projects'][2]
        self.assertIn('cycle 9',track['task']['text'])
        self.assertIn('Inspect this project',track['task']['text'])

    def test_current_owned_cycle_counts_as_active_not_waiting(self):
        cycles=[{'dir':'000001','files':{'result.json':{'started_utc':'2026-01-01T10:00:00Z','ended_utc':'2026-01-01T10:30:00Z'}}},
                {'dir':'000002','files':{'result.json':{'sequence':2,'status':'running','started_utc':'2026-01-01T11:00:00Z'}}}]
        status={'sequence':2,'status':'running','current':{'sequence':2}}
        result=domain.build_time(cycles,status,dt.datetime(2026,1,1,12,tzinfo=dt.timezone.utc))
        self.assertEqual(result['active_seconds'],5400)
        self.assertEqual(result['waiting_seconds'],1800)
        self.assertFalse(result['partial'])

    def test_abandoned_unfinished_cycle_is_not_counted_as_active(self):
        cycles=[{'dir':'000001','files':{'result.json':{'sequence':1,'status':'running','started_utc':'2026-01-01T10:00:00Z'}}}]
        result=domain.build_time(cycles,{'sequence':2,'status':'waiting'},dt.datetime(2026,1,1,12,tzinfo=dt.timezone.utc))
        self.assertIsNone(result['active_seconds'])
        self.assertTrue(result['partial'])
