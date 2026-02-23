# Details

**Full documentation:** https://webawesome.com/docs/components/details


`<wa-details>` Since 2.0 stable

Details show a brief summary and expand to show additional content.

```html
<wa-details summary="Toggle Me">
  Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
  aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
</wa-details>
```

## Examples

### Disabled

Use the `disabled` attribute to prevent the details from expanding.

```html
<wa-details summary="Disabled" disabled>
  Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
  aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
</wa-details>
```

### Customizing the Summary Icon

Use the `expand-icon` and `collapse-icon` slots to change the expand and collapse icons, respectively. To disable the animation, override the `rotate` property on the `icon` part as shown below.

```html
<wa-details summary="Toggle Me" class="custom-icons">
  <wa-icon name="square-plus" slot="expand-icon" variant="regular"></wa-icon>
  <wa-icon name="square-minus" slot="collapse-icon" variant="regular"></wa-icon>

  Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
  aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
</wa-details>

<style>
  /* Disable the expand/collapse animation */
  wa-details.custom-icons::part(icon) {
    rotate: none;
  }
</style>
```

### Icon Position

The default position for the expand and collapse icons is at the end of the summary. Set the `icon-placement` attribute to `start` to place the icon at the start of the summary.

```html
<div class="wa-stack">
  <wa-details summary="Start" icon-placement="start">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
  <wa-details summary="End" icon-placement="end">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
</div>
```

### HTML in Summary

To use HTML in the summary, use the `summary` slot. Links and other interactive elements will still retain their behavior:

```html
<wa-details>
  <span slot="summary">
    Some text
    <a href="https://webawesome.com" target="_blank">a link</a>
    more text
  </span>

  Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
  aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
</wa-details>
```

### Right-to-Left Languages

The details component, including its `icon-placement`, automatically adapts to right-to-left languages:

```html
<div class="wa-stack">
  <wa-details summary="تبديلني" lang="ar" dir="rtl">
    استخدام طريقة لوريم إيبسوم لأنها تعطي توزيعاَ طبيعياَ -إلى حد ما- للأحرف عوضاً عن
  </wa-details>
  <wa-details summary="تبديلني" lang="ar" dir="rtl" icon-placement="start">
    استخدام طريقة لوريم إيبسوم لأنها تعطي توزيعاَ طبيعياَ -إلى حد ما- للأحرف عوضاً عن
  </wa-details>
</div>
```

### Appearance

Use the `appearance` attribute to change the element’s visual appearance.

```html
<div class="wa-stack">
  <wa-details summary="Outlined (default)">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
  <wa-details summary="Filled-outlined" appearance="filled-outlined">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
  <wa-details summary="Filled" appearance="filled">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
  <wa-details summary="Plain" appearance="plain">
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>
</div>
```

### Grouping Details

Use the `name` attribute to create accordion-like behavior where only one details element with the same name can be open at a time. This matches the behavior of native `<details>` elements.

```html
<div class="wa-stack">
  <wa-details name="group-1" summary="Section 1" open>
    Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna
    aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
  </wa-details>

  <wa-details name="group-1" summary="Section 2">
    Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam,
    eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo.
  </wa-details>

  <wa-details name="group-1" summary="Section 3">
    At vero eos et accusamus et iusto odio dignissimos ducimus qui blanditiis praesentium voluptatum deleniti atque
    corrupti quos dolores et quas molestias excepturi sint occaecati cupiditate non provident.
  </wa-details>
</div>
```

## Importing

Autoloading components via [projects](https://webawesome.com/docs/#using-a-project) is the recommended way to import components. If you prefer to do it manually, use one of the following code snippets.

\*\*CDN\*\*

Let your project code do the work! [Sign up for free](https://webawesome.com/signup) to use a project with your very own CDN — it's the fastest and easiest way to use Web Awesome.

\*\*npm\*\*

To manually import this component from NPM, use the following code.

```js
import '@awesome.me/webawesome/dist/components/details/details.js';
```

\*\*React\*\*

To manually import this component from React, use the following code.

```js
import WaDetails from '@awesome.me/webawesome/dist/react/details';
```

## Slots

Learn more about [using slots](https://webawesome.com/docs/usage/#slots).

| Name | Description |
| --- | --- |
| (default) | The details' main content. |
| \`collapse-icon\` | \`\` Optional expand icon to use instead of the default. Works best with . |
| \`summary\` | \`summary\` The details' . Alternatively, you can use the summary attribute. |

## Attributes & Properties

Learn more about [attributes and properties](https://webawesome.com/docs/usage/#attributes-and-properties).

| Name | Description | Reflects |
| --- | --- | --- |
| \`appearance\` appearance | \`'filled' \\| 'outlined' \\| 'filled-outlined' \\| 'plain'\` The element's visual appearance. Type Default 'outlined' | | |
| \`css\` | \`CSSResultGroup \\| undefined\` One or more CSSResultGroup to include in the component's shadow root. Host styles are automatically prepended. Type Default styles | | |
| \`disabled\` disabled | \`boolean\` Disables the details so it can't be toggled. Type Default false | | |
| \`iconPlacement\` icon-placement | \`'start' \\| 'end'\` The location of the expand/collapse icon. Type Default 'end' | | |
| \`name\` name | \`string\` Groups related details elements. When one opens, others with the same name will close. Type | | |
| \`open\` open | \`show()\` Indicates whether or not the details is open. You can toggle this attribute to show and hide the details, or you can use the and hide() methods and this attribute will reflect the details' open state. Type boolean Default false | | |
| \`summary\` summary | \`summary\` The to show in the header. If you need to display HTML, use the summary slot instead. Type string | | |

## Methods

Learn more about [methods](https://webawesome.com/docs/usage/#methods).

| Name | Description | Arguments |
| --- | --- | --- |
| \`hide()\` | Hides the details | |
| \`show()\` | Shows the details. | |

## Events

Learn more about [events](https://webawesome.com/docs/usage/#events).

| Name | Description |
| --- | --- |
| \`wa-after-hide\` | Emitted after the details closes and all animations are complete. |
| \`wa-after-show\` | Emitted after the details opens and all animations are complete. |
| \`wa-hide\` | Emitted when the details closes. |
| \`wa-show\` | Emitted when the details opens. |

## CSS custom properties

Learn more about [CSS custom properties](https://webawesome.com/docs/customizing/#custom-properties).

| Name | Description |
| --- | --- |
| \`--hide-duration\` | \`200ms\` The hide duration to use when applying built-in animation classes. Default |
| \`--show-duration\` | \`200ms\` The show duration to use when applying built-in animation classes. Default |
| \`--spacing\` | The amount of space around and between the details' content. Expects a single value. |

## Custom States

Learn more about [custom states](https://webawesome.com/docs/customizing/#custom-states).

| Name | Description | CSS selector |
| --- | --- | --- |
| \`animating\` | Applied when the details is animating expand/collapse. | \`:state(animating)\` |

## CSS parts

Learn more about [CSS parts](https://webawesome.com/docs/customizing/#css-parts).

| Name | Description | CSS selector |
| --- | --- | --- |
| \`base\` | \`\` The inner element used to render the component. Styles you apply to the component are automatically applied to this part, so you usually don't need to deal with it unless you need to set the display property. | \`::part(base)\` |
| \`content\` | The details content. | \`::part(content)\` |
| \`header\` | The header that wraps both the summary and the expand/collapse icon. | \`::part(header)\` |
| \`icon\` | The container that wraps the expand/collapse icons. | \`::part(icon)\` |
| \`summary\` | The container that wraps the summary. | \`::part(summary)\` |

## Dependencies

This component automatically imports the following elements. Sub-dependencies, if any exist, will also be included in this list.

-   [`<wa-icon>`](https://webawesome.com/docs/components/icon)

**Need a hand?** Report a bug Ask for help