import '@fontsource-variable/geist'
import '@fontsource-variable/geist-mono'
import '@fontsource-variable/bricolage-grotesque'
import { IconContext } from '@phosphor-icons/react'
import { MotionConfig } from 'motion/react'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { initMode } from './api'
import { Gate } from './companion'
import App from './App'
import { Provider } from './store'
import './styles.css'

initMode().then(() => {
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <MotionConfig reducedMotion="user">
        <IconContext.Provider value={{ weight: 'light', size: 18 }}>
          <BrowserRouter><Gate><Provider><App /></Provider></Gate></BrowserRouter>
        </IconContext.Provider>
      </MotionConfig>
    </StrictMode>,
  )
})
