import lazy router from '@ohos.router';

@Entry
@Component
struct Index {
  @State message: string = 'Hello ArkTS';

  build() {
    Column() {
      Text(this.message)
        .fontSize(24)
      Button('Open')
        .onClick(() => {
          router.pushUrl({ url: 'pages/Detail' });
        })
    }
  }
}

@interface RouteMeta {
  name: string;
}
