import { BellRinging } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useApp } from '../store'
import { SPRING } from './Motion'

/** Small in-app explainer shown once, right after the first job starts, before the browser permission dialog. */
export function NotifPrompt() {
  const { askOs, answerOs } = useApp()
  return (
    <AnimatePresence>
      {askOs && (
        <motion.div className="nprompt" role="dialog" aria-label="Desktop notifications" initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 20 }} transition={SPRING}>
          <BellRinging size={22} aria-hidden />
          <div><b>Get a heads-up when it is done?</b><p>Tailoring takes a minute. We can show a desktop notification when a job finishes or needs you. It shows the job name only, never your resume text.</p>
            <div className="row"><button className="btn sm primary" onClick={() => answerOs(true)}>Turn on</button><button className="btn sm ghost" onClick={() => answerOs(false)}>Not now</button></div></div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
