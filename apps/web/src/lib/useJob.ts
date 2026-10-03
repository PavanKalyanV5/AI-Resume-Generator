import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../api'
import { useApp } from '../store'
import { isActive, type JobDetail } from '../types'

export function useJob(id: string) {
  const { watch, events } = useApp()
  const [data, setData] = useState<JobDetail | null>(null)
  const [error, setError] = useState('')
  const seen = useRef(id)
  const load = useCallback(async () => {
    try { const d = await api.getJob(id); if (seen.current === id) { setData(d); setError('') } }
    catch (e) { setError(e instanceof Error ? e.message : 'Could not load this job.') }
  }, [id])
  useEffect(() => { seen.current = id; setData(null); setError(''); load(); return watch(id) }, [id, load, watch])
  const n = events[id]?.length ?? 0
  useEffect(() => { const t = setTimeout(load, 120); return () => clearTimeout(t) }, [n, load])
  const active = data ? isActive(data.job.status) : false
  useEffect(() => { if (!active) return; const t = setInterval(load, 3000); return () => clearInterval(t) }, [active, load])
  return { data, error, reload: load }
}
