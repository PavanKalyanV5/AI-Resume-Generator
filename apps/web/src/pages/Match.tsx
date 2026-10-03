import { Rise, CountUp } from '../components/Motion'
import { ClusterMap, CoverageBars, Gauge, Heatmap, KnowledgeGraph, MatchLegend, SkillClusters, scoreOf, strengths } from '../components/Charts'
import { gapHint, gapText } from '../lib/util'
import { useJobCtx } from './JobDetail'

export default function Match() {
  const { data } = useJobCtx()
  const { coverage, graph, gaps, clusters } = data
  if (!graph.nodes.length) return <div className="empty"><h2>Match report not ready</h2><p>It appears after the job description is parsed and your experience is ranked.</p></div>
  const semantic = coverage.semantic ?? []
  const rows = strengths(graph.nodes, graph.edges, coverage.covered, semantic)
  const nSem = rows.filter((r) => r.kind === 'semantic').length, nKw = rows.filter((r) => r.kind === 'keyword').length, nMiss = rows.filter((r) => r.kind === 'missing').length
  const p = scoreOf(coverage.score)
  return (
    <div className="stack gap-xl">
      <div className="match-top">
        <Rise as="section" className="panel" i={0}>
          <header><h2>ATS coverage</h2><p>How much of the job description your tailored resume speaks to.</p></header>
          <Gauge score={coverage.score} />
          <div className="trio">
            <div><CountUp value={nKw} /><span>by keyword</span></div>
            <div className="sem"><CountUp value={nSem} /><span>by meaning</span></div>
            <div className="miss"><CountUp value={nMiss} /><span>missing</span></div>
          </div>
          <p className="small muted">{nKw + nSem} of {rows.length} requirements covered{p < 75 ? ', so a few gaps remain below.' : '.'}</p>
        </Rise>
        <Rise as="section" className="panel" i={1}>
          <header><h2>Requirement by requirement</h2><p>Bar length is the strongest evidence found. Hatched bars were matched by meaning, not by the same words.</p></header>
          <MatchLegend nKw={nKw} nSem={nSem} nMiss={nMiss} />
          <CoverageBars rows={rows} />
        </Rise>
      </div>
      <Rise as="section" className="panel" inView>
        <header><h2>Which bullets answer which requirement</h2><p>Empty rows are requirements nothing in your resume speaks to.</p></header>
        <Heatmap nodes={graph.nodes} edges={graph.edges} rows={rows} />
      </Rise>
      {clusters && clusters.points.length > 0 && (
        <Rise as="section" className="panel" inView>
          <header><h2>Topic map of your bullets</h2><p>Bullets that say similar things sit close together. Hover a point to read the bullet, or pick a topic to isolate it.</p></header>
          <ClusterMap data={clusters} />
        </Rise>
      )}
      <Rise as="section" className="panel" inView>
        <header><h2>Knowledge graph</h2><p>Select a node to see what it connects to. Drag to rearrange. Dashed red rings mark requirement gaps.</p></header>
        <KnowledgeGraph graph={graph} missing={coverage.missing} />
      </Rise>
      <div className="cols">
        <Rise as="section" className="panel" inView><header><h2>Skills</h2></header>
          <SkillClusters resume={data.tailored ?? data.original} covered={coverage.covered} semantic={semantic} missing={coverage.missing} />
        </Rise>
        <Rise as="section" className="panel" inView i={1}><header><h2>Gaps to close</h2></header>
          {gaps.length === 0 ? <p className="muted">No gaps found.</p> : (
            <ul className="clean gaps">{gaps.map((g, i) => (
              <li key={i}><b>{gapText(g)}</b>{gapHint(g) && <span className="small muted">{gapHint(g)}</span>}</li>
            ))}</ul>
          )}
        </Rise>
      </div>
    </div>
  )
}
