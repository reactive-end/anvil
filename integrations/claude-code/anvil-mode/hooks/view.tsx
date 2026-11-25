import type { EngineInterface } from 'claude-code'

import type { Catalog, Effort, Mode } from '../types'
import { findModel, levelsOf, resolveModel } from './catalog'
import { effortName, modelName } from './i18n'
import type { Strings } from './i18n'

type Ui = ReturnType<EngineInterface['ui']['resolve']>

export type Actions = {
  mode: (enabled: boolean) => void
  model: (model: string) => void
  level: (effort: Effort) => void
}

type Props = { ui: Ui; t: Strings; catalog: Catalog; now: Mode; act: Actions }

/** The catalog in order, main then overflow; a current model it lacks is listed too. */
function modelOptions(catalog: Catalog, t: Strings, current: string) {
  const ordered = [
    ...catalog.models.filter(model => model.section === 'main'),
    ...catalog.models.filter(model => model.section === 'overflow'),
  ]
  const options = ordered.map(model => ({ value: model.id, label: model.name }))

  if (findModel(catalog, current) === undefined) {
    options.push({ value: current, label: modelName(current, t) })
  }

  return options
}

/** The executor's model and effort; the orchestrator runs on the session's own picker. */
function executor({ ui, t, catalog, now, act }: Props) {
  const { Box, Select } = ui
  const current = now.executor
  const levels = levelsOf(catalog, current.model).map(level => level.id)
  // A stored level the model does not list stays an option, so the pill is never blank.
  const efforts = levels.includes(current.effort) ? levels : [...levels, current.effort]

  return (
    <Box key="executor" columnGap={1} alignItems="center">
      <Select
        key="executor-model"
        label={t.executorRole}
        value={resolveModel(catalog, current.model)}
        options={modelOptions(catalog, t, resolveModel(catalog, current.model))}
        onSelect={value => act.model(value)}
      />
      {levels.length > 0 && (
        <Select
          key="executor-effort"
          value={current.effort}
          options={efforts.map(id => ({ value: id, label: effortName(id, t) }))}
          onSelect={value => act.level(value as Effort)}
        />
      )}
    </Box>
  )
}

export function band(p: Props) {
  const { Box, Button } = p.ui
  const { t, now, act } = p

  return (
    // Numeric gaps: the named steps are refused by the engine this was tested on.
    <Box flexWrap="wrap" alignItems="center" columnGap={4} rowGap={1}>
      {now.enabled && executor(p)}
      <Button key="mode" onPress={() => act.mode(!now.enabled)}>
        {now.enabled ? t.turnOff : t.turnOn}
      </Button>
    </Box>
  )
}
