// All data here is FAKE. Names, employers and numbers are invented for the demo.
import type { Clusters, GraphEdge, GraphNode, LibraryItem, Resume } from './types'

export const ORIGINAL: Resume = {
  profile: { name: 'Jane Doe', email: 'jane.doe@example.test', phone: '+1 555 0100', role: 'Senior Robotics Software Engineer', location: 'Portland, OR' },
  socials: [{ label: 'GitHub', url: 'https://example.test/janedoe' }],
  summary: ['Robotics software engineer with eight years of experience in navigation, control and fleet tooling.'],
  experience: [
    {
      id: 'acme-sr', role: 'Senior Robotics Software Engineer', organization: 'Acme Robotics', client: 'Globex Logistics', location: 'Portland, OR', dateLabel: 'Mar 2021 - Present',
      bullets: [
        'Led a team of five building the navigation stack for warehouse robots in C++17 and ROS 2, cutting path replan latency from 180 ms to 60 ms.',
        'Wrote a model predictive controller for differential-drive bases running at a 200 Hz control loop on a real-time Linux kernel.',
        'Built a fleet telemetry pipeline with Prometheus and Grafana that surfaced battery and drive faults across 300 robots.',
        'Set up GitHub Actions pipelines that run hardware-in-the-loop tests nightly and gate every release.',
      ],
    },
    {
      id: 'acme-eng', role: 'Robotics Software Engineer', organization: 'Acme Robotics', client: 'Globex Logistics', location: 'Portland, OR', dateLabel: 'Aug 2018 - Feb 2021',
      bullets: [
        'Ported the perception pipeline from ROS 1 to ROS 2 with zero downtime for deployed customers at Globex Logistics.',
        'Mentored four junior engineers through code review and weekly pairing, two of whom were promoted.',
        'Wrote Python tooling to replay field logs and reproduce navigation failures locally.',
      ],
    },
    {
      id: 'northwind', role: 'Controls Engineer', organization: 'Northwind Automation', location: 'Tacoma, WA', dateLabel: 'Jun 2015 - Jul 2018',
      bullets: ['Tuned PID loops for conveyor and pick-and-place cells, improving throughput by 11 percent.'],
    },
  ],
  education: [{ school: 'State Polytechnic University', degree: 'B.S. Electrical Engineering', year: '2015' }],
  skills: [
    { label: 'Languages', skills: ['C++17', 'Python', 'Bash'] },
    { label: 'Robotics', skills: ['ROS 2', 'Nav2', 'Model predictive control', 'PID tuning'] },
    { label: 'Infrastructure', skills: ['Docker', 'GitHub Actions', 'Prometheus', 'Grafana', 'Linux'] },
  ],
  projects: [], certifications: [],
}

export const TAILORED: Resume = {
  ...ORIGINAL,
  summary: ['Senior robotics software engineer with eight years shipping C++17 and ROS 2 navigation and real-time control to production warehouse fleets.'],
  experience: [
    {
      ...ORIGINAL.experience[0],
      bullets: [
        'Led a team of five building the ROS 2 navigation stack in C++17 for warehouse robots, cutting path replan latency from 180 ms to 60 ms.',
        'Wrote a real-time model predictive controller for differential-drive bases, running a 200 Hz control loop on a PREEMPT_RT Linux kernel.',
        'Built fleet telemetry and observability with Prometheus and Grafana that surfaced battery and drive faults across 300 robots.',
        'Set up CI/CD in GitHub Actions that runs hardware-in-the-loop tests nightly and gates every fleet release.',
      ],
    },
    {
      ...ORIGINAL.experience[1],
      bullets: [
        'Mentored four junior engineers through code review and weekly pairing, two of whom were promoted.',
        'Ported the ROS 1 perception pipeline to ROS 2 in production with zero downtime for deployed customers at Globex Logistics.',
        'Wrote Python tooling to replay field logs and reproduce navigation failures locally.',
      ],
    },
    ORIGINAL.experience[2],
  ],
  skills: [
    { label: 'Languages', skills: ['C++17', 'Python', 'Bash'] },
    { label: 'Robotics', skills: ['ROS 2', 'Nav2', 'Model predictive control', 'Real-time control', 'PID tuning'] },
    { label: 'Infrastructure', skills: ['CI/CD', 'GitHub Actions', 'Docker', 'Prometheus', 'Grafana', 'Real-time Linux'] },
  ],
}

const reqs: [string, boolean][] = [
  ['C++17 or later', true], ['ROS 2 in production', true], ['Real-time motion control', true], ['CI/CD for robot fleets', true],
  ['Python tooling', false], ['Mentoring engineers', false], ['Fleet telemetry and observability', true],
  ['Kubernetes at scale', true], ['Functional safety (ISO 13849)', true], ['Rust', false], ['Simulation with Isaac Sim', false],
]
const b = ORIGINAL.experience.flatMap((e) => e.bullets)
const skillNames = ['C++17', 'ROS 2', 'Python', 'Docker', 'GitHub Actions', 'Prometheus', 'MPC', 'Linux']

export const REQ_COVERED = reqs.slice(0, 7).map((r) => r[0])
export const REQ_MISSING = reqs.slice(7).map((r) => r[0])

export const GRAPH: { nodes: GraphNode[]; edges: GraphEdge[] } = {
  nodes: [
    ...reqs.map(([label, required], i) => ({ id: `r${i + 1}`, kind: 'requirement', label, required })),
    ...b.map((label, i) => ({ id: `b${i + 1}`, kind: 'bullet', label })),
    ...skillNames.map((label, i) => ({ id: `s${i + 1}`, kind: 'skill', label })),
    { id: 'o1', kind: 'org', label: 'Acme Robotics' }, { id: 'o2', kind: 'org', label: 'Northwind Automation' },
    { id: 'ro1', kind: 'role', label: 'Senior Robotics Software Engineer' }, { id: 'ro2', kind: 'role', label: 'Robotics Software Engineer' }, { id: 'ro3', kind: 'role', label: 'Controls Engineer' },
    { id: 'p1', kind: 'project', label: 'Fleet fault dashboard' }, { id: 'c1', kind: 'cert', label: 'Certified ROS Developer' },
  ],
  edges: [
    ['b1', 'r1', 0.9], ['b1', 'r2', 0.8], ['b2', 'r3', 0.95], ['b2', 'r1', 0.3], ['b3', 'r7', 0.9], ['b4', 'r4', 0.85], ['b4', 'r7', 0.2],
    ['b5', 'r2', 0.9], ['b6', 'r6', 0.95], ['b7', 'r5', 0.9], ['b8', 'r3', 0.4], ['b3', 'r4', 0.15],
  ].map(([from, to, weight]) => ({ from: from as string, to: to as string, kind: 'covers', weight: weight as number })).concat(
    [['s1', 'r1', 0.9], ['s2', 'r2', 0.9], ['s3', 'r5', 0.8], ['s4', 'r4', 0.4], ['s5', 'r4', 0.8], ['s6', 'r7', 0.8], ['s7', 'r3', 0.7], ['s8', 'r3', 0.5]]
      .map(([from, to, weight]) => ({ from: from as string, to: to as string, kind: 'mentions', weight: weight as number })),
    ['b1', 'b2', 'b3', 'b4', 'b5', 'b6', 'b7'].map((x) => ({ from: x, to: 'o1', kind: 'at', weight: 0.3 })),
    [{ from: 'b8', to: 'o2', kind: 'at', weight: 0.3 }],
    ['b1', 'b2', 'b3', 'b4'].map((x) => ({ from: 'ro1', to: x, kind: 'contains', weight: 0.4 })),
    ['b5', 'b6', 'b7'].map((x) => ({ from: 'ro2', to: x, kind: 'contains', weight: 0.4 })),
    [{ from: 'ro3', to: 'b8', kind: 'contains', weight: 0.4 }, { from: 'ro1', to: 'o1', kind: 'at', weight: 0.3 }, { from: 'ro2', to: 'o1', kind: 'at', weight: 0.3 }, { from: 'ro3', to: 'o2', kind: 'at', weight: 0.3 }],
    [{ from: 'r1', to: 's1', kind: 'requires', weight: 0.5 }, { from: 'r2', to: 's2', kind: 'requires', weight: 0.5 }, { from: 'r8', to: 's4', kind: 'similarto', weight: 0.45 }, { from: 'r4', to: 'r8', kind: 'similarto', weight: 0.35 },
      { from: 'p1', to: 's6', kind: 'mentions', weight: 0.5 }, { from: 'p1', to: 'r7', kind: 'covers', weight: 0.5 }, { from: 'c1', to: 's2', kind: 'mentions', weight: 0.5 }],
  ),
}

export const GAPS = [
  { requirement: 'Kubernetes at scale', suggestion: 'Your Docker and fleet pipeline work is adjacent. Add a line if you have run workloads on a cluster.' },
  { requirement: 'Functional safety (ISO 13849)', suggestion: 'No safety standards appear in your resume. Mention them only if you have worked to them.' },
  { requirement: 'Rust', suggestion: 'Listed as nice to have. Skip unless you have shipped Rust code.' },
  { requirement: 'Simulation with Isaac Sim', suggestion: 'Nothing on simulation yet. Gazebo or similar experience would count as related.' },
]

const redact = (s: string) =>
  s.replaceAll('Jane Doe', '[PERSON_1]').replaceAll('jane.doe@example.test', '[EMAIL_1]').replaceAll('+1 555 0100', '[PHONE_1]')
    .replaceAll('Globex Logistics', '[CLIENT_1]').replaceAll('Acme Robotics', '[ORG_1]').replaceAll('Northwind Automation', '[ORG_2]')

export const AUDIT = {
  redacted_payload: {
    system: 'You are a resume editor. Rewrite the supplied bullets so they speak to the job requirements. Do not invent facts, employers or numbers. Keep every token such as [ORG_1] exactly as written.',
    user: redact(JSON.stringify({
      candidate: ORIGINAL.profile.name, contact: [ORIGINAL.profile.email, ORIGINAL.profile.phone],
      requirements: REQ_COVERED.concat(REQ_MISSING),
      summary: ORIGINAL.summary,
      experience: ORIGINAL.experience.map((e) => ({ id: e.id, role: e.role, organization: e.organization, client: e.client, bullets: e.bullets })),
    }, null, 2)),
  },
  token_counts: { PERSON: 1, EMAIL: 1, PHONE: 1, ORG: 4, CLIENT: 3, CUSTOM: 0 } as Record<string, number>,
}

export const DEMO_JD = `Senior Robotics Software Engineer, Fleet Navigation

We build autonomous mobile robots for warehouses. You will own parts of our navigation and control stack and help teams ship safely.

Requirements
- C++17 or later and ROS 2 in production
- Real-time motion control (MPC or similar)
- CI/CD for robot fleets, including hardware-in-the-loop testing
- Fleet telemetry and observability
- Kubernetes at scale
- Functional safety experience (ISO 13849)
- Mentoring engineers

Nice to have: Python tooling, Rust, simulation with Isaac Sim.`

export const SEMANTIC = ['Real-time motion control', 'Fleet telemetry and observability']

const CL = [
  { id: 'nav', label: 'Navigation and ROS 2', size: 4 }, { id: 'ctl', label: 'Real-time control', size: 3 },
  { id: 'ops', label: 'Fleet, CI and telemetry', size: 4 }, { id: 'lead', label: 'Leadership and tooling', size: 3 },
]
const extra = [
  'Cut map-update bandwidth by 40 percent by sending only changed grid tiles to each robot.',
  'Profiled and removed priority inversions in the drive-control thread on PREEMPT_RT Linux.',
  'Added alerting on motor temperature drift that paged on-call before drives failed in the field.',
  'Ran design reviews for the navigation team and wrote the onboarding guide for new hires.',
]
const allB = [...b, ...extra]
const pos: [string, number, number][] = [
  ['nav', -0.62, 0.48], ['nav', -0.45, 0.7], ['ctl', 0.52, 0.55], ['ctl', 0.7, 0.3], ['ops', 0.35, -0.55], ['ops', 0.1, -0.7],
  ['lead', -0.55, -0.4], ['ctl', 0.4, 0.78], ['nav', -0.8, 0.28], ['ops', 0.58, -0.4], ['ops', 0.25, -0.35], ['lead', -0.3, -0.62],
]
// bullets 1..8 were assigned by hand to the nearest theme
const order = [0, 3, 2, 4, 1, 6, 11, 7, 8, 5, 9, 10]
export const CLUSTERS: Clusters = {
  clusters: CL,
  points: order.map((bi, i) => ({ id: `cp${i + 1}`, label: allB[bi] ?? allB[0], x: pos[i][1], y: pos[i][2], cluster: pos[i][0] })),
}

const D = 864e5
const lib = (id: string, company: string, role: string, days: number, coverage: number | null, drive: boolean): LibraryItem => ({
  id, company, role, created_at: new Date(Date.now() - days * D).toISOString(), coverage,
  files: [{ name: 'resume.pdf', display_name: `Jane Doe - ${company}.pdf` }, { name: 'resume.docx', display_name: `Jane Doe - ${company}.docx` }],
  drive: drive ? [{ name: `Jane Doe - ${company}.pdf`, url: 'https://drive.google.com/file/d/demo' }, { name: `Jane Doe - ${company}.docx`, url: 'https://drive.google.com/file/d/demo2' }] : null,
})
export const LIBRARY: LibraryItem[] = [
  lib('demo-seed-1', 'Globex Logistics', 'Senior Robotics Software Engineer', 1.2, 0.68, true),
  lib('demo-seed-2', 'Initech Autonomy', 'Staff Controls Engineer', 1.1, 0.81, false),
  lib('demo-seed-3', 'Globex Logistics', 'Fleet Platform Engineer', 2.1, 0.57, true),
  lib('demo-seed-5', 'Umbrella Dynamics', 'Robotics Tech Lead', 4.1, 0.74, false),
  lib('demo-seed-6', 'Hooli Warehousing', 'Navigation Engineer', 6.2, 0.49, false),
  { ...lib('demo-seed-4', 'Initech Autonomy', 'Perception Engineer', 3.1, null, false), files: [] },
]
