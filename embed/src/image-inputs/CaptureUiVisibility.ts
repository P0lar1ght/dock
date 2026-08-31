/** Temporarily hides the embedded Agent UI so it cannot cover host pixels being selected or captured. */
export function hideEmbeddedAgentUi(documentValue: Document = document) {
  const agents = [...documentValue.querySelectorAll<HTMLElement>('dock-agent')]
    .map((element) => ({
      element,
      value: element.style.getPropertyValue('visibility'),
      priority: element.style.getPropertyPriority('visibility')
    }));
  for (const { element } of agents) {
    element.style.setProperty('visibility', 'hidden', 'important');
  }
  return () => {
    for (const { element, value, priority } of agents) {
      if (value) element.style.setProperty('visibility', value, priority);
      else element.style.removeProperty('visibility');
    }
  };
}
